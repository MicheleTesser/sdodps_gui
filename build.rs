use std::env;
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const DEFAULT_DBC_PATH: &str = "dbc/can2.dbc";
const DEFAULT_DBCC_DIR: &str = "dbc/dbcc";

fn main() {
    println!("cargo:rerun-if-env-changed=SDODPS_DBC_PATH");
    println!("cargo:rerun-if-env-changed=SDODPS_DBCC_PATH");
    println!("cargo:rerun-if-changed=build.rs");

    let manifest_dir = PathBuf::from(required_env("CARGO_MANIFEST_DIR"));
    let out_dir = PathBuf::from(required_env("OUT_DIR"));
    let dbc_path = resolve_path(
        &manifest_dir,
        env::var_os("SDODPS_DBC_PATH")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(DEFAULT_DBC_PATH)),
    );

    watch(&dbc_path);
    let dbcc = match env::var_os("SDODPS_DBCC_PATH") {
        Some(path) => resolve_path(&manifest_dir, PathBuf::from(path)),
        None => {
            let source_dir = manifest_dir.join(DEFAULT_DBCC_DIR);
            watch_dbcc_sources(&source_dir);
            build_dbcc(&source_dir);
            source_dir.join("dbcc")
        }
    };

    if !dbc_path.is_file() {
        fail(format!("DBC non trovato: {}", dbc_path.display()));
    }
    if !dbcc.is_file() {
        fail(format!("dbcc non trovato: {}", dbcc.display()));
    }

    let output = run(
        Command::new(&dbcc)
            .arg("-R")
            .arg("-o")
            .arg(&out_dir)
            .arg(&dbc_path),
        "generazione Rust con dbcc -R",
    );
    require_success(output, "dbcc -R");

    let generated_name = dbc_path
        .file_stem()
        .and_then(OsStr::to_str)
        .map(|stem| format!("{stem}.rs"))
        .unwrap_or_else(|| "can2.rs".to_string());
    let generated_path = out_dir.join(generated_name);
    if !generated_path.is_file() {
        fail(format!(
            "dbcc -R non ha creato il modulo atteso {}",
            generated_path.display()
        ));
    }

    let wrapper_path = out_dir.join("sdodps_generated_module.rs");
    fs::write(
        &wrapper_path,
        format!(
            "#[allow(dead_code, clippy::all)]\n#[path = {:?}]\nmod codec;\n",
            generated_path
        ),
    )
    .unwrap_or_else(|error| {
        fail(format!(
            "impossibile creare il wrapper 2rust {}: {error}",
            wrapper_path.display()
        ))
    });
    println!(
        "cargo:rustc-env=SDODPS_GENERATED_WRAPPER_RS={}",
        wrapper_path.display()
    );
    println!(
        "cargo:rustc-env=SDODPS_DBC_SOURCE={}",
        canonical(&dbc_path).display()
    );
}

fn required_env(name: &str) -> String {
    env::var(name).unwrap_or_else(|_| fail(format!("variabile Cargo {name} assente")))
}

fn resolve_path(manifest_dir: &Path, path: PathBuf) -> PathBuf {
    if path.is_absolute() {
        path
    } else {
        manifest_dir.join(path)
    }
}

fn canonical(path: &Path) -> PathBuf {
    fs::canonicalize(path)
        .unwrap_or_else(|error| fail(format!("impossibile risolvere {}: {error}", path.display())))
}

fn watch(path: &Path) {
    println!("cargo:rerun-if-changed={}", path.display());
}

fn watch_dbcc_sources(directory: &Path) {
    watch(&directory.join("makefile"));
    let entries = fs::read_dir(directory).unwrap_or_else(|error| {
        fail(format!(
            "sorgenti dbcc non disponibili in {}: {error}; inizializza i submodule",
            directory.display()
        ))
    });
    for entry in entries.flatten() {
        let path = entry.path();
        let watched = matches!(path.extension().and_then(OsStr::to_str), Some("c" | "h"));
        if watched {
            watch(&path);
        }
    }
}

fn build_dbcc(source_dir: &Path) {
    let output = run(
        Command::new("make").arg("-C").arg(source_dir).arg("dbcc"),
        "compilazione di dbcc",
    );
    require_success(output, "make dbcc");
}

fn run(command: &mut Command, action: &str) -> Output {
    command
        .output()
        .unwrap_or_else(|error| fail(format!("{action} non avviabile: {error}")))
}

fn require_success(output: Output, action: &str) {
    if output.status.success() {
        return;
    }
    fail(format!(
        "{action} fallito ({}):\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    ));
}

fn fail(message: String) -> ! {
    panic!("{message}")
}
