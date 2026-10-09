# Validación de entrega

Entorno: Fedora/Linux, Rust/Cargo 1.98.0, Node 24.15.0 local al proyecto,
OpenCode 1.18.35, curl 8.18.0. Validación realizada el 9 de octubre de 2026.

## Resultados

| Control | Resultado |
|---|---|
| `cargo build --release --locked` | PASS |
| `cargo fmt --check` | PASS |
| `cargo clippy --locked --all-targets -- -D warnings` | PASS, sin advertencias |
| `cargo test --locked` | 30 pruebas: 1 unitaria + 29 de integración, PASS |
| `python3 tests/operations.py` | 7 pruebas operativas, PASS |
| Ruff formato y lint de scripts/tests Python | PASS |
| TypeScript `tsc --noEmit` | PASS |
| Prettier de plugin y renderer compartido | PASS |
| Node tests (vista + JSON Schema contra CLI Rust) | 5 pruebas, PASS |
| GNOME con mocks de lifecycle/menú/caché | PASS |
| GJS/Gio nativo, lectura asíncrona y renderer compartido | PASS |
| Plugin mínimo en OpenCode real, PTY aislada | PASS |
| Plugin completo en OpenCode real: barra, cambio de caché, `/quota` | PASS |
| Plugin desde ubicación instalada en staging, sin node_modules copiados | PASS |
| Instalación/desinstalación en staging y `systemd-analyze --user verify` | PASS |
| `npm audit` con lockfile y overrides de desarrollo | 0 vulnerabilidades reportadas |
| Pruebas originales, compiladas con target y temporales en el proyecto nuevo | Codex 4 + Go 6, PASS |
| Smoke GNOME de ambos proyectos originales, solo lectura | 2, PASS |

Los tests cubren porcentajes, ventanas múltiples, valores parciales/ausentes,
credenciales inexistentes/expiradas, HTTP 401/403/429, Retry-After numérico y fecha,
JSON inválido, desconexión, timeout real Codex de 25 segundos y recolección de su
proceso hijo, obsolescencia, ventanas vencidas, lectores concurrentes, escritores
excluidos, recuperación durable, metadata común y reinicio/backoff.

Las pruebas operativas verifican SIGTERM, SIGKILL, exclusión de segundo daemon,
recuperación de caché sin nueva consulta prematura, curl simulado sin secreto en
argv/salida, importación de SQLite con WAL sin modificar origen, idempotencia,
staging/uninstall preservando config, composición de PS1 Bash, `check` sin escritura,
reinicios relativos calculados después de la respuesta y avisos urgentes, deduplicados
y acotados con un notificador simulado. No se enviaron notificaciones reales.

El JSON Schema se compila independientemente con Ajv y valida salidas reales de
la CLI, tanto sin muestras como con una adquisición simulada completa.

## Rendimiento

100 ejecuciones del binario release con caché local: **mediana 1,07 ms, p95 1,53 ms,
máximo 2,23 ms**, incluyendo creación del proceso. No se hicieron peticiones de red.
Reproducible con `python3 scripts/benchmark.py`, después de ejecutar la demo.
Es una medición de esta máquina, no una garantía para otros sistemas.

## Reproducir

Núcleo y operaciones:

```bash
cargo build --release --locked
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
python3 tests/operations.py
```

Suite completa (Node/npm instalados localmente o disponibles en PATH):

```bash
python3 -m venv target/test-env
target/test-env/bin/pip install -r tests/requirements.txt
npm --prefix integrations/opencode ci --ignore-scripts
scripts/check.sh
```

Prueba TUI real: comandos en [opencode.md](opencode.md). Requiere OpenCode y pyte;
no depende de una cuenta autenticada ni envía prompts al modelo fixture.

Prueba GJS:

```bash
python3 scripts/demo.py
gjs -m tests/gjs.mjs sandbox/cache/status.json
```

## Verificación externa pendiente

- Consulta del nuevo binario contra las cuentas reales Codex/Go: adaptadores probados
  con mocks; las cachés originales mostraron éxito en la auditoría, pero eso no
  sustituye una consulta real de la nueva implementación.
- Habilitación, apariencia y compatibilidad exacta de la extensión dentro de tu
  sesión GNOME. Se verificaron mocks y GJS/Gio nativo, no se habilitó la extensión.
- Activación del nuevo servicio dentro de tu manager systemd real y suspensión
  física del equipo. Se validó unidad en staging, procesos reales aislados y avance
  de reloj simulado para reanudación; no se suspendió tu sesión.
- Starship y Zsh: ejemplos entregados; estos ejecutables no estaban instalados en
  el PATH inspeccionado. Bash sí fue probado.
- Entrega de notificaciones por el escritorio: lógica y deduplicación probadas;
  no se enviaron alertas a tu sesión durante las pruebas.

Los dos servicios originales permanecieron activos, con cero reinicios registrados.
`ai-quota-monitor.service` no está instalado ni habilitado en la sesión real.
No se cambiaron archivos fuente originales, configuraciones de shell/OpenCode ni
credenciales. Las instalaciones y pruebas visuales se hicieron en destinos nuevos
dentro del proyecto.
