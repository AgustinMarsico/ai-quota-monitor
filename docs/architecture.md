# Arquitectura

## Decisiones

Rust conserva la lógica correcta de ambos monitores, ofrece un ejecutable rápido
y evita un runtime para el prompt. Dependencias del núcleo: Serde/JSON, SQLite
bundled, libc para señales/umask y httpdate para `Retry-After` HTTP-date.
No hay runtime async ni servidor HTTP permanente. `curl` reutiliza el transporte
Fedora existente y recibe la cabecera de autenticación por stdin.

```text
providers::{codex,go} → Reading / ProviderError
                         ↓
scheduler → normalized model → SQLite transaction → atomic JSON
                ↓                                  ↓
          alert state                     CLI, shell, OpenCode, GNOME
```

- `providers`: transporte, autenticación, parsing, fuentes y limitaciones.
- `model`: tipos normalizados, validación y obsolescencia.
- `config`: único JSON tipado; settings desconocidos generan error.
- `storage`: bloqueos, SQLite, snapshot, historial, CSV e importación.
- `scheduler`: intervalos por perfil, backoff, señales y notificaciones.
- `cli`: argumentos, salida rápida y diagnóstico offline.
- `integrations/shared/view.mjs`: validación y representación visual común.

Para añadir un proveedor, implementar `Provider::fetch`, registrar el identificador
y agregar fixtures. El scheduler, persistencia y contrato no dependen de sus ventanas.

## Contrato público

[`schema/status-v1.schema.json`](../schema/status-v1.schema.json) valida la salida
Rust con Ajv en una prueba independiente. Se admite agregar campos en v1;
cambios incompatibles requieren otra `schema_version`. Los consumidores rechazan
versiones desconocidas y datos incompletos, sin sustituciones por cero.

Timestamps: enteros Unix UTC en segundos. `generated_at` es el momento del último
ciclo persistido; **no** garantiza un nuevo dato remoto. `last_success_at` registra
la recepción exitosa; el proveedor puede haber devuelto su propio estado cacheado.

Cada perfil contiene proveedor, cuenta/modelo/plan opcionales, fuente, limitaciones,
último intento/éxito y lista de cuotas con ambos porcentajes, duración y reinicio.
La ventana mensual no se expresa como una cantidad inventada de minutos.

Estados:
- `fresh`: muestra válida dentro de TTL y sin ventana vencida ni último error.
- `stale`: conserva cuotas, pero hubo error, vencimiento, antigüedad o reloj futuro.
- `unavailable`: todavía no hay muestra ni error.
- `error`: intento fallido sin cuotas disponibles.

El estado es conservador a nivel de perfil: una ventana vencida marca el perfil
como obsoleto. El reinicio real siempre requiere otra lectura del proveedor.

## Consistencia y recuperación

`daemon.lock` evita dos daemons; `writer.lock` excluye todos los escritores,
incluidos refresh e importación. Se libera explícitamente en RAII, incluso tras
errores; el kernel lo libera tras muerte del proceso. Los bloqueos usan archivos
privados en el directorio de estado, no se borran mientras el sistema funciona.

SQLite WAL guarda muestra, deduplicación de alertas y snapshot durable en una
transacción. Después se escribe un temporal 0600, se hace fsync, rename y fsync
del directorio. Los lectores nunca ven JSON parcial. Si el proceso muere entre
commit y publicación, el siguiente ciclo reconstruye la caché desde SQLite.
`status` no repara ni escribe datos: si hay caché corrupta, informa error.

`next_attempt_at` y errores sobreviven reinicios. Solo hay una consulta pendiente
por perfil. El daemon revisa vencimientos cada cinco segundos y no reescribe la
caché en ciclos inactivos. Tras suspensión consulta lo vencido una vez, sin
reproducir todos los intervalos omitidos. Los transportes son secuenciales y
acotados a 25 s (Codex) / 20 s (Go), una decisión deliberada de simplicidad.

Backoff exponencial con jitter acotado; errores de autenticación usan una base
de al menos diez minutos. `Retry-After`, numérico o fecha HTTP, tiene prioridad
sobre el máximo normal de backoff. No se hacen reintentos internos ocultos.

Alertas 20/10/5: un aviso por salto al nivel más urgente, con estado durable.
Reinicios Go estimados admiten una tolerancia de cinco minutos. En ventanas con
reinicio conocido no se repite un umbral del mismo período. Sin reinicio conocido,
se rearman únicamente tras observar recuperación, nunca por un día artificial.

SQLite no contiene claves ni respuestas remotas completas. Logs del daemon se
limitan a fallos resumidos una vez por minuto; journalctl aporta el diagnóstico.
