# Validación y pruebas aisladas

Los comandos de esta página operan sobre fixtures y directorios temporales, no
sobre la instalación ni la base SQLite del servicio activo. Las pruebas con
`curl` simulado no publican en ntfy.sh ni consultan cuentas reales. Para ejecutar
el conjunto completo se requieren Rust/Cargo, Python 3, Ruff, Node/npm y,
para los tests nativos GNOME, GJS y las bibliotecas de GNOME Shell.

## Verificación del árbol de trabajo (24 de octubre de 2026)

| Control | Resultado |
|---|---|
| `cargo fmt --check` | PASS |
| `cargo clippy --locked --offline --all-targets -- -D warnings` | PASS |
| `env -u OPENCODE_API_KEY cargo test --locked --offline` | PASS; 84 tests Rust (incluida la prueba FIFO OAuth y ambos ejemplos de configuración) |
| `python3 tests/operations.py` | PASS; 11 pruebas de procesos y transportes simulados |
| `python3 tests/oauth_operations.py` | PASS; 3 pruebas OAuth aisladas |
| Ruff, formato y lint de scripts/tests | PASS después de alinear el formato |
| TypeScript y Prettier | PASS con Node local ya disponible |
| Plugin OpenCode Node | PASS; 5 tests |
| GNOME Node | PASS; presentación y lifecycle |
| GNOME GJS nativo, schema en memoria | PASS en staging, GNOME Shell 50.6 |
| `git diff --check` | PASS |

Los tests nuevos de `tests/policy_storage.rs` cubren SQLite previa, migración
repetida sin replay, duración distinta, 100% histórico, deduplicación tras
reinicio, modo desactivado y confirmación de recuperación interrumpida por fallo
o ventana ausente. `tests/operations.py` verifica que `status` y `check` no
escriben, que `notifications test` no toca SQLite, que `writer.lock` se libera
antes de entregar y que los fallos ntfy no alteran las consultas.

```bash
cargo fmt --check
cargo clippy --locked --offline --all-targets -- -D warnings
env -u OPENCODE_API_KEY cargo test --locked --offline
python3 tests/operations.py
python3 tests/oauth_operations.py
git diff --check
```

Si ya están disponibles localmente Ruff y Node (no instalar paquetes para
ejecutar una auditoría offline), ejecutar también:

```bash
target/test-env/bin/ruff format --check scripts tests
target/test-env/bin/ruff check --cache-dir target/ruff-cache scripts tests
npm --prefix integrations/opencode run typecheck
npm --prefix integrations/opencode run lint
npm --prefix integrations/opencode test
node tests/gnome.mjs
```

`scripts/check.sh` reúne las verificaciones del núcleo, Python, OpenCode y GNOME
Node; requiere esas herramientas en `PATH`. La prueba GJS nativa es separada:

```bash
python3 scripts/install_gnome.py --destination target/gnome-test/ai-quota@local
env GSETTINGS_BACKEND=memory \
  GI_TYPELIB_PATH=/usr/lib64/gnome-shell:/usr/lib64/mutter-18 \
  LD_LIBRARY_PATH=/usr/lib64/gnome-shell:/usr/lib64/mutter-18 \
  gjs -m tests/gjs-gnome.mjs target/gnome-test/ai-quota@local/schemas
```

Esas rutas GNOME son propias de Fedora con GNOME Shell 50; ajustarlas según el
sistema. La validación aislada no sustituye probar voluntariamente la apariencia
en una sesión GNOME real ni la entrega a un topic propio.

## Registro histórico

La validación inicial del **9 de octubre de 2026** (antes de OAuth Console,
políticas avanzadas y la actualización GNOME) registró 30 tests Rust, 7 tests
operativos y una mediana de **1,07 ms**, p95 **1,53 ms**, para 100 ejecuciones
release de `status` contra caché local. Es una medición de aquella máquina, no
un resultado de rendimiento nuevo ni una garantía. Se reproduce, si se desea,
con `python3 scripts/benchmark.py` tras preparar la demo. Las validaciones de
un servicio o cuenta real requieren decisión del operador y no se infieren de
esos fixtures.
