# sdodps_gui

TUI `ratatui` per navigare e comandare variabili SDO partendo dal DBC RaceUP.

Caratteristiche principali:

- parsing automatico dei messaggi `SDO*` dal DBC
- config file con `dbc_path` e `socketcan` di default
- override da CLI con `--dbc`, `--can`, `--dbcc`
- filtro per scheda, ricerca per nome variabile, ordinamento asc/desc
- `get`, `get all`, `set`
- log/ultima operazione sempre visibile
- gestione `SIG_VALTYPE_` per `float32/float64`, oltre a signed/unsigned
- integrazione runtime `dbcc` + `bindgen`

## Requisiti

- Rust toolchain
- Linux con SocketCAN
- `clang` / `libclang` per `bindgen`
- `dbcc` compilato in `dbc/dbcc/dbcc`

## Setup

In questa repo il submodule `dbcc` è annidato dentro `dbc`.

```bash
git -C dbc submodule update --init --recursive
make -C dbc/dbcc
```

## Avvio

Al primo avvio viene creato `sdodps_gui.toml`.

```bash
cargo run
```

Con override:

```bash
cargo run -- --dbc dbc/can2.dbc --can can1 --dbcc dbc/dbcc/dbcc
```

Esempio di config:

```toml
dbc_path = "dbc/can2.dbc"
socketcan = "can0"
dbcc_path = "dbc/dbcc/dbcc"
```

## Tasti

- `Tab` / `Shift+Tab`: cambia pannello
- `Up` / `Down`: naviga
- `f`: filtra sulla scheda selezionata
- `a`: rimuove il filtro scheda
- `/`: filtro testuale variabili
- `s`: cambia colonna di ordinamento
- `r`: inverte direzione ordinamento
- `g`: getter sulla variabile selezionata
- `G`: getter su tutte le variabili visibili
- `e` o `Enter`: setter sulla variabile selezionata
- `Esc`: esce da input o resetta i filtri
- `q`: esce

## Note sul protocollo

- Per le schede `SDO*`, il tool deriva automaticamente `board`, `var_id`, tipo e payload dal DBC.
- L’integrazione `dbcc`/`bindgen` genera output runtime in `/tmp/sdodps_gui_dbcc/<nome_dbc>/`.
- Se viene passato un DBC diverso, il parser Rust continua a funzionare; se `dbcc` è disponibile, vengono rigenerati anche gli artefatti runtime.

## Verifica locale

```bash
cargo check
cargo test
```
