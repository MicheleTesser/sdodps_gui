# VIBECODED

Questa repo e' stata estesa con una CLI orientata a operazioni SDO ripetibili e scriptabili.

Comandi principali:

```bash
cargo run -- list
cargo run -- list --board PCU
cargo run -- get --board PCU --variable kp_batt
cargo run -- set --board PCU --variable kp_batt --value 1.25
cargo run -- export
cargo run -- export --board PCU --output exports/pcu.toml
cargo run -- restore exports/pcu.toml
cargo run -- restore exports/pcu.toml --board PCU
```

Garanzie introdotte:

- restore stabile per `board + variable + type`
- export singola scheda o globale
- restore singola scheda o globale
- `set` con attesa ack dal bus
- mismatch di tipo e variabili mancanti riportati esplicitamente

API raw usate sul bus:

- opcode `1`: `GET`
- opcode `2`: `SET`
- opcode `128`: response/ack
- opcode `253`: out of range
- opcode `254`: readonly
- opcode `255`: generic error

Dettagli completi ed esempi stanno in [README.md](README.md).
