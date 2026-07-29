# sdodps_gui

CLI e TUI per leggere, esportare e riscrivere variabili SDO partendo dal DBC RaceUP.

La CLI e' ora il path principale per le operazioni batch:

- export di tutte le schede o di una singola scheda
- restore di tutte le schede o di una singola scheda
- `get` di una singola variabile
- `set` di una singola variabile con attesa ack
- elenco schede e variabili note nel DBC

Il match in restore non dipende dall'ordine nel DBC. Ogni entry viene risolta con:

- nome scheda
- nome variabile
- tipo (`u8`, `i16`, `f32`, ecc.)

Quindi se sposti variabili o ne inserisci altre in mezzo, il restore non riassegna valori alla voce sbagliata.

## Requisiti

- Rust toolchain
- Linux con SocketCAN
- toolchain C e `make` per compilare `dbcc`

## Setup

```bash
git -C dbc submodule update --init --recursive
cargo build
```

`build.rs` compila automaticamente `dbc/dbcc/dbcc`, esegue `dbcc -R` e
include nel binario il modulo Rust generato da `dbc/can2.dbc`. Non sono più
necessari `bindgen`, `clang` o binding C runtime.

Per compilare un DBC diverso:

```bash
SDODPS_DBC_PATH=path/to/vehicle.dbc cargo build
```

È possibile indicare un eseguibile `dbcc` già pronto:

```bash
SDODPS_DBCC_PATH=/path/to/dbcc cargo build
```

## Config

Al primo avvio viene creato `sdodps_gui.toml`.

Esempio:

```toml
dbc_path = "dbc/can2.dbc"
socketcan = "can0"
```

`dbc_path` deve contenere lo stesso DBC incorporato durante la build. La GUI
verifica i byte all'avvio e rifiuta un file differente, evitando di codificare
frame con un layout diverso dal modulo `2rust`. Per cambiare DBC occorre quindi
ricompilare con `SDODPS_DBC_PATH`.

Override globali supportati da tutti i comandi:

- `--config <file>`
- `--dbc <file>`
- `--can <iface>`
- `--timeout-ms <ms>`

## CLI

Sintassi generale:

```bash
cargo run -- [override globali] <comando> [opzioni]
```

### List

Elenca le schede SDO note nel DBC:

```bash
cargo run -- list
```

Elenca le variabili di una scheda:

```bash
cargo run -- list --board PCU
```

Output colonne:

- `board`
- `variable`
- `type`
- `var_id`

### Get

Legge una singola variabile e aspetta la risposta dal bus:

```bash
cargo run -- get --board PCU --variable kp_batt
```

### Set

Scrive una singola variabile e aspetta l'ack dal nodo.

```bash
cargo run -- set --board PCU --variable kp_batt --value 1.25
```

Il comando e' considerato riuscito solo se arriva una risposta valida per la stessa `board` e `var_id` entro `--timeout-ms`.

### Export

Esporta tutte le schede:

```bash
cargo run -- export
```

Esporta una singola scheda:

```bash
cargo run -- export --board PCU
```

Esporta in un file specifico:

```bash
cargo run -- export --board PCU --output exports/pcu_setup.toml
```

L'export CLI legge davvero le variabili dal bus con una sequenza di `get`, non usa cache locale.

### Restore

Ripristina tutto il file:

```bash
cargo run -- restore exports/export_all_1712345678.toml
```

Ripristina solo una scheda dal file:

```bash
cargo run -- restore exports/garage.toml --board PCU
```

Per ogni entry il tool:

- trova la scheda per nome
- trova la variabile per nome
- verifica che il tipo coincida
- invia `set`
- aspetta ack

Se una entry non esiste piu' o il tipo e' cambiato, viene segnalata a fine run.

## Formato export

I file vengono creati sotto `exports/` e sono ignorati dal git.

Esempio:

```toml
version = 1

[[entries]]
board = "PCU"
variable = "kp_batt"
type_name = "f32"
var_id = 7
unit = "-"

[entries.value]
kind = "float"
value = 1.25
```

`var_id` viene salvato solo a scopo informativo. Il restore non si basa su quello.

## TUI

La TUI resta disponibile con:

```bash
cargo run
```

Tasti principali:

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
- `x`: esporta i valori attualmente in cache TUI
- `Esc`: esce da input o resetta i filtri
- `q`: esce

## Chiamate protocollo SDO

Il layout dei frame, gli opcode, i tipi wire, scaling e offset provengono dal
modulo generato da `2rust`. La CLI usa queste operazioni sul bus CAN:

- `GET request`
  - payload 7 byte
  - bit `0..8`: opcode `1`
  - bit `8..18`: `var_id`
- `SET request`
  - payload 7 byte
  - bit `0..8`: opcode `2`
  - bit `8..18`: `var_id`
  - bit `24..`: valore codificato secondo il tipo DBC
- `ACK/response`
  - opcode `128`: risposta valida con payload valore
  - opcode `253`: out of range
  - opcode `254`: write readonly
  - opcode `255`: errore generico

Per `set`, l'ack e' una risposta sullo stesso CAN ID della scheda e sulla stessa `var_id`.

## Verifica locale

```bash
cargo check
cargo test
make -C dbc/dbcc test_rust
```
