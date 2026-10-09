# AI Quota Monitor

Monitor unificado de cuotas **Codex y OpenCode Go**, con CLI rápida, daemon único,
SQLite, caché JSON versionada e interfaces OpenCode y GNOME. Implementado en Rust
para Linux; no requiere ejecutar ninguno de los monitores originales.

```text
Codex 5h: 78% | Codex 7d: 62% | Go 5h: 91% | Go 7d: 45% | Go month: 93%
```

Los valores representan **porcentaje restante**. `~` indica datos obsoletos;
`?`, datos no disponibles. El monitor nunca supone que una ventana vencida
restauró la cuota. Fechas ausentes son `null`, no cero.

## Compilar y probar sin afectar la sesión

Requisitos del núcleo: Rust >= 1.89, Cargo, compilador C/enlazador (GCC), Python 3 para scripts/pruebas.
En ejecución real: Codex CLI autenticado con ChatGPT, `curl` para Go;
`notify-send` es opcional para alertas de escritorio.

```bash
cd ~/Descargas/monitor/ai-quota-monitor
cargo build --release --locked
python3 scripts/demo.py
./target/release/ai-quota --config sandbox/config.json --cache sandbox/cache/status.json status --compact
./target/release/ai-quota --config sandbox/config.json --cache sandbox/cache/status.json status --json
./target/release/ai-quota --config sandbox/config.json --state-dir sandbox/state --cache sandbox/cache/status.json export
```

La demo usa dos transportes simulados, desactiva notificaciones y escribe solamente
en `sandbox/`. No consulta servicios externos ni instala unidades systemd.

## CLI

```bash
ai-quota status
ai-quota status --json
ai-quota status --compact --color never
ai-quota status --provider codex
ai-quota status --provider opencode-go
ai-quota status --compact --metric used
ai-quota refresh
ai-quota check --provider codex
ai-quota doctor --json
ai-quota history --days 7
ai-quota export --days 30 > cuotas.csv
ai-quota daemon
```

**`status` no hace red, no inicia proveedores y no abre SQLite.** Lee la caché y
recalcula obsolescencia según su edad. `refresh` es una consulta explícita; respeta
el backoff incluso cuando se solicita manualmente. El daemon y `refresh` comparten
bloqueo y estado de planificación. Un segundo daemon falla de forma explícita.

`check` recupera el diagnóstico remoto de los monitores originales: no escribe
caché, historial ni estado, y no envía notificaciones. Usa el bloqueo existente
cuando hay un monitor y respeta el backoff guardado. Los errores nuevos de `check`
no se persisten; `refresh` es el comando que registra intentos y planificación.

Opciones comunes: `--config PATH`, `--state-dir PATH`, `--cache PATH`,
`--provider ID`, `--color never|auto|always`, `--metric remaining|used`.
`NO_COLOR` desactiva colores. Exit codes: 0 éxito; 1 consulta fallida o en backoff
con error; 2 configuración, argumentos, caché o almacenamiento inválidos.

## Configuración y datos

- Configuración: `$XDG_CONFIG_HOME/ai-quota/config.json`, normalmente `~/.config/ai-quota/config.json`.
- Estado durable: `$XDG_STATE_HOME/ai-quota/usage.sqlite3`.
- Caché: `$XDG_CACHE_HOME/ai-quota/status.json`.
- Archivo de ejemplo: [`config/example.json`](config/example.json).
- Credenciales Go: `$XDG_DATA_HOME/opencode/auth.json`, o ruta `auth_file` por perfil.
- Codex usa su autenticación existente. Puede configurarse `codex_bin` y `codex_home`.
- `OPENCODE_API_KEY` es un override opcional; afecta todos los perfiles Go del proceso.

Los proveedores pueden desactivarse y tener perfiles independientes. Un perfil
Codex adicional necesita su propio `codex_home`; Go, su propio `auth_file`.
Cuenta y modelo permanecen `null` cuando no se informan. El plan se conserva
únicamente si Codex lo entrega. No se infieren cuotas por modelo.

No se elimina historial por defecto: `retention_days: null`. Si configurás un
entero, autorizás la retención de ese número de días **en la base nueva**.

## Interfaces e instalación

- [Instalación y systemd](docs/installation.md)
- [Migración e historial](docs/migration.md)
- [Bash, Zsh y Starship](docs/terminal.md)
- [Plugin visual OpenCode](docs/opencode.md)
- [Arquitectura y contrato JSON](docs/architecture.md)
- [Seguridad, auditoría y límites](docs/audit.md)
- [Pruebas y verificaciones](docs/validation.md)

El instalador copia archivos; **no activa servicios ni extensiones** y no cambia
configuraciones de shell u OpenCode. La activación es un paso explícito posterior.

## Desarrollo

```bash
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
env -u OPENCODE_API_KEY cargo test --locked
python3 tests/operations.py
```

Para interfaces, Node >=24.15 y npm (solo desarrollo):

```bash
npm --prefix integrations/opencode ci --ignore-scripts
npm --prefix integrations/opencode run typecheck
npm --prefix integrations/opencode run lint
npm --prefix integrations/opencode test
node tests/gnome.mjs
```

Para ejecutar todo con `scripts/check.sh`, preparar además los verificadores Python:

```bash
python3 -m venv target/test-env
target/test-env/bin/pip install -r tests/requirements.txt
scripts/check.sh
```

Licencia MIT. Atribuciones de lógica reutilizada en [`NOTICE`](NOTICE).
