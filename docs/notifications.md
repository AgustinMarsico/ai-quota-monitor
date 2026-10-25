# Notificaciones

Las alertas de cuota se detectan y deduplican en el núcleo, independientemente del
destino. SQLite conserva el estado de deduplicación; los transportes solo reciben
eventos ya aceptados por la política. Un error al publicar no altera la consulta
de cuotas, el backoff del proveedor ni su código de salida.

## Compatibilidad y migración

Sin `notification_policy` (o con `"notification_policy": null`),
`"notifications": true` conserva la política legacy: escritorio (`notify-send`),
umbrales de 80%, 90% y 95% **consumido**, deduplicación por ventana y período,
y sin eventos nuevos. La política y los transportes son independientes:
`"notifications": false` desactiva la entrega y no crea workers aunque haya
política configurada. `status` y `check` nunca envían notificaciones. ntfy está
desactivado a menos que se configure expresamente.

Para migrar, añadí `notification_policy` de manera explícita; no hace falta
reinstalar ni editar la base SQLite a mano. Antes de activarla, revisá los
umbrales y eventos: los valores por defecto de la política nueva **no son**
los del modo legacy. Conservá una copia de tu configuración para poder volver
a la política anterior quitando `notification_policy`. El historial de muestras
y la caché de estado siguen siendo independientes de la entrega de mensajes;
no se garantiza una notificación retroactiva por muestras anteriores al cambio.

En el primer `refresh`/ciclo del daemon tras actualizar, `open()` crea de modo
idempotente las tablas `notification_quota_state` (identidad proveedor, perfil,
ventana y duración; estado JSON versionado) y `notification_health_state`
(proveedor y perfil; incidente JSON versionado). La creación es transaccional y
no borra `alerts`, `samples` ni historial. El primer estado nuevo se basa en las
marcas legacy y la última lectura durable de la misma ventana y duración para
no reproducir alertas anteriores, incluido un 100% ya observado. Las lecturas
futuras pueden activar umbrales superiores. Un binario anterior no interpreta
los nuevos episodios: guardá un respaldo privado de la SQLite antes de una
actualización manual y no esperes que un downgrade preserve la deduplicación
de eventos nuevos.

Para actualizar manualmente una instalación existente, **fuera de esta sesión**:
respaldá configuración y SQLite privada (con el servicio detenido, incluyendo
WAL si existe), compilá `cargo build --locked --offline --release`, reemplazá
el binario de usuario mediante el procedimiento habitual de instalación y
reiniciá `systemctl --user restart ai-quota-monitor.service`. Verificá con
`systemctl --user status ai-quota-monitor.service`,
`journalctl --user -u ai-quota-monitor.service -n 100 --no-pager`,
`ai-quota status` y, solo si querés publicar una prueba real al transporte,
`ai-quota notifications test`. La configuración no se recarga en caliente.

## Política configurable (fases 4–6)

Ejemplo de configuración en `config.json` (integrar las claves en la
configuración existente y ajustar perfiles/ventanas a los configurados):

```json
{
  "notifications": true,
  "notification_policy": {
    "thresholds_used_percent": [80, 90, 95, 100],
    "overrides": [
      { "provider": "codex", "thresholds_used_percent": [90, 95, 100] },
      { "provider": "codex", "profile": "default", "thresholds_used_percent": [80, 95, 100] },
      { "provider": "codex", "profile": "default", "window": "primary", "thresholds_used_percent": [95, 100] }
    ],
    "events": {
      "threshold": true,
      "quota_renewed": true,
      "quota_recovered": true,
      "monitor_failed": false,
      "monitor_recovered": false
    },
    "recovery": { "margin_percent": 5, "confirmations": 2 },
    "monitor_failure": {
      "consecutive_attempts": 3,
      "minimum_duration_seconds": 600
    }
  }
}
```

`thresholds_used_percent` expresa **porcentaje consumido** (no restante), en
orden ascendente sin duplicados, mayores que 0 y hasta 100. Los overrides
sustituyen la lista completa, no la combinan. La precedencia es global →
proveedor → perfil → ventana, con coincidencia exacta; un override de ventana
requiere proveedor y opcionalmente perfil (sin perfil, alcanza a todos los
perfiles del proveedor). Ventanas conocidas: Codex `primary`, `secondary`;
OpenCode Go `rolling`, `weekly`, `monthly`. Los perfiles deben existir en
`providers`.
La lista global y cada override deben tener al menos un umbral; las reglas
duplicadas o los nombres de proveedor/ventana desconocidos son inválidos.

`events` controla qué tipos se entregan: `threshold` cuando el consumo cruza
un umbral, `quota_renewed` cuando se confirma un período nuevo,
`quota_recovered` cuando se confirma una recuperación de cuota,
`monitor_failed` cuando el monitor acumula fallos reales y
`monitor_recovered` cuando vuelve a funcionar tras un fallo notificado. Los
eventos de cuota son por proveedor/perfil/ventana; los de monitor son por
proveedor/perfil. Desactivar un evento no cambia los porcentajes consultados
ni convierte un fallo de entrega en un fallo del proveedor. `threshold`,
`quota_renewed` y `quota_recovered` están activados por defecto en la política
nueva; los eventos de salud del monitor están desactivados por defecto.

`recovery.margin_percent` establece la separación respecto del umbral para
considerar que el consumo se alejó de la zona de alerta;
`recovery.confirmations` exige ese número de lecturas exitosas consecutivas
antes de confirmar recuperación y rearmar avisos. La recuperación no se
deduce del mero vencimiento de una ventana ni de un reloj local. Por defecto
son 5 puntos porcentuales y 2 confirmaciones. `monitor_failure` exige **ambas**
condiciones desde el primer intento fallido real: al menos
`consecutive_attempts` intentos consecutivos y
`minimum_duration_seconds` segundos transcurridos (por defecto 3 y 600).
Un ciclo omitido por backoff no cuenta como intento. Un éxito posterior puede
dar lugar a `monitor_recovered` si hubo un fallo confirmado.

La configuración rechaza propiedades desconocidas. `confirmations` admite
2–100 y `consecutive_attempts`, 1–100; `margin_percent`, más de 0 y hasta 100;
`minimum_duration_seconds`, 0–86400. Conviene activar primero la política
con `notifications: false`, validar el JSON y después habilitar
manualmente los eventos y transportes deseados.

El archivo [`config/notifications.example.json`](../config/notifications.example.json)
incluye proveedores y perfiles de ejemplo y es validado por las pruebas Rust.
Mantiene ntfy deshabilitado: el topic es un placeholder, no un destino real.

## ntfy opt-in

Ejemplo sin autenticación ni secretos (reemplazar el topic por el propio):

```json
{
  "notifications": true,
  "notification_transports": {
    "desktop": true,
    "ntfy": {
      "enabled": true,
      "server_url": "https://ntfy.sh",
      "topic": "REEMPLAZAR_POR_TOPIC_EXISTENTE",
      "token_file": null
    }
  }
}
```

El topic es configuración, no una credencial formal. En un topic público,
cualquier persona que lo conozca puede potencialmente suscribirse o publicar.
No incluyas información personal en los nombres de perfil. Para un topic
protegido, `token_file` puede apuntar a un archivo regular del usuario, con
permisos `0600` o más restrictivos y sin enlaces duros. El token se lee solo si
el transporte está habilitado. Nunca se registra ni se incluye en argumentos.

El transporte construye un payload JSON ntfy con título, ventana, proveedor,
perfil, consumo, porcentaje disponible y renovación solo si se informó. La
configuración de `curl` y el payload se envían por stdin usando `curl --config -`.
No se siguen redirecciones ni se desactiva la validación TLS.

## Entrega

Cada transporte activo tiene un worker, un canal acotado de 32 mensajes y un
buffer temporal acotado de hasta 32 mensajes mientras espera reintentos. Cada
evento vence a los 10 minutos desde su creación, medidos con reloj monotónico.
El daemon no espera por HTTP mientras monitorea ni mantiene `writer.lock` durante
la entrega. Cada intento ntfy tiene límites de conexión y duración; se hacen
hasta tres intentos para fallos transitorios mientras el proceso continúa.
Errores permanentes HTTP no se reintentan. La cola y los reintentos son en memoria:
si el equipo se apaga, se pierde un mensaje pendiente. Al llenarse la cola se
descarta sin bloquear. Una respuesta perdida después de aceptación remota puede
ocasionar duplicados. La garantía es best-effort, no exactly-once ni entrega
durable.

En `refresh`, la espera inicial de entrega es breve: un ntfy lento o un 429
puede agotar el presupuesto del proceso antes de completar reintentos; la
deduplicación ya confirmada no reenvía automáticamente el evento. Asimismo,
un fallo al publicar la caché después del commit SQLite puede impedir entregar
el aviso de ese ciclo, aunque el siguiente ciclo repare la caché. Si se exige
entrega durable ante esas situaciones, haría falta diseñar una outbox; no se
promete esa garantía en esta versión.

Las APIs públicas `scheduler::update_with()` y `update_with_events()` también
inician el runtime configurado y entregan alertas best-effort. Los llamadores
que necesitan persistir sin entregar pueden usar explícitamente
`update_with_events_without_notifications()`.

`refresh` es un proceso de una sola ejecución: espera un período acotado para que
los workers comiencen la entrega y luego los cierra. Un envío que aún esté en
curso puede alargar el cierre hasta el timeout del transporte. El daemon mantiene
los workers entre ciclos y los cierra explícitamente al recibir SIGTERM/SIGINT.

Los fallos de transporte se informan de forma genérica a journald; no incluyen
topic, token, cuerpo de respuesta ni payload.

## Probar transportes

Ejecutá `ai-quota notifications test` para enviar una notificación claramente
marcada como prueba a cada transporte habilitado. El comando usa la misma
configuración de escritorio/ntfy y espera un resultado de entrega con un límite
de 12 segundos. Informa `accepted`, `failed`, `timed_out` o `not_enabled`; el
código de salida es distinto de cero si algún envío falla, vence o no hay ningún
transporte habilitado. No consulta proveedores ni escribe cache, SQLite,
historial o estado del daemon. La salida nunca incluye topic, token ni payload.

Este comando **solo prueba el transporte**, no simula cruces de umbrales,
renovaciones, recuperación ni fallos del monitor. Para probar la política
manualmente sin tocar instalación ni datos reales, ejecutá primero
`python3 scripts/demo.py`, copiá `sandbox/config.json` a
`sandbox/policy.json` y añadí ahí la política del ejemplo, manteniendo
`"notifications": false`. La demo **reescribe** `sandbox/config.json` cada vez;
no pongas cambios que quieras conservar en ese archivo. Ejecutá después:

```bash
env -u OPENCODE_API_KEY PATH="$PWD/sandbox/bin:/usr/bin:/bin" \
  ./target/release/ai-quota --config sandbox/policy.json \
  --state-dir sandbox/policy-state --cache sandbox/policy-cache/status.json refresh
```

Usá la ruta del binario que compilaste (`target/debug/ai-quota` si corresponde).
El fixture estático de la demo permite validar configuración y una consulta
aislada, pero **no** demuestra por sí mismo transiciones de umbral, renovación,
recuperación o fallo. Para comprobar transiciones hacen falta respuestas de
fixture sucesivas y consultas efectivas (respetando el backoff); `status`,
`check` y `notifications test` no disparan eventos de política. Habilitá
manualmente un transporte solo si necesitás verificar la entrega, sin
confundir recepción con corrección: los envíos pueden perderse o duplicarse.
