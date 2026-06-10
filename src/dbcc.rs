use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};

use crate::config::RuntimeConfig;

#[derive(Debug, Clone)]
pub struct DbccRuntime {
    pub executable: Option<PathBuf>,
    pub generated_dir: Option<PathBuf>,
    pub bindings_path: Option<PathBuf>,
    pub status: String,
}

impl DbccRuntime {
    pub fn prepare(config: &RuntimeConfig) -> Result<Self> {
        let executable = config.dbcc_path.clone().or_else(|| find_in_path("dbcc"));

        let Some(executable) = executable else {
            return Ok(Self {
                executable: None,
                generated_dir: None,
                bindings_path: None,
                status: "dbcc non trovato: parser Rust attivo, bindgen runtime disabilitato"
                    .to_string(),
            });
        };

        let dbc_stem = config
            .dbc_path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or("dbc");
        let generated_dir = PathBuf::from("/tmp/sdodps_gui_dbcc").join(dbc_stem);
        fs::create_dir_all(&generated_dir)
            .with_context(|| format!("failed to create {}", generated_dir.display()))?;

        let output = Command::new(&executable)
            .arg("-o")
            .arg(&generated_dir)
            .arg(&config.dbc_path)
            .output()
            .with_context(|| format!("failed to run {}", executable.display()))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Ok(Self {
                executable: Some(executable),
                generated_dir: Some(generated_dir),
                bindings_path: None,
                status: format!("dbcc fallito: {}", stderr.trim()),
            });
        }

        let header = find_generated_header(&generated_dir);
        let bindings_path = match header {
            Some(ref header) => {
                let bindings_path = generated_dir.join("bindings.rs");
                let bindings = bindgen::Builder::default()
                    .header(header.to_string_lossy())
                    .generate_comments(false)
                    .generate()
                    .context("bindgen generation failed")?;
                bindings
                    .write_to_file(&bindings_path)
                    .with_context(|| format!("failed to write {}", bindings_path.display()))?;
                Some(bindings_path)
            }
            None => None,
        };

        let status = match (&header, &bindings_path) {
            (Some(header), Some(bindings)) => format!(
                "dbcc pronto: header {} -> bindings {}",
                header.display(),
                bindings.display()
            ),
            _ => format!(
                "dbcc eseguito in {}, ma nessun header .h rilevato per bindgen",
                generated_dir.display()
            ),
        };

        Ok(Self {
            executable: Some(executable),
            generated_dir: Some(generated_dir),
            bindings_path,
            status,
        })
    }
}

fn find_in_path(name: &str) -> Option<PathBuf> {
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|path| path.join(name))
            .find(|candidate| candidate.exists())
    })
}

fn find_generated_header(directory: &Path) -> Option<PathBuf> {
    fs::read_dir(directory)
        .ok()?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .find(|path| path.extension().and_then(|ext| ext.to_str()) == Some("h"))
}
