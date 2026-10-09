# Migración sin perder configuración

1. Compilar, ejecutar la demo y revisar la instalación en staging.
2. Instalar archivos nuevos (no activar todavía).
3. Adaptar el config nuevo: intervalos, Codex ejecutable/home y auth_file de Go.
   Los overrides existentes siguen intactos; no se leen ni copian secretos del servicio.
4. Importar historial, conservando siempre las bases originales.
5. Verificar una consulta real **explícita** y decidir el cambio de servicio.
6. Activar el nuevo monitor y retirar los anteriores solo cuando decidas hacerlo.

## Importación

```bash
ai-quota migrate --provider codex --from "$HOME/.local/state/codex-quota"
ai-quota migrate --provider opencode-go --from "$HOME/.local/state/opencode-go-quota"
ai-quota history --days 30
ai-quota export --days 3650 > cuotas-importadas.csv
```

Respeta XDG si reemplazás las rutas por tus ubicaciones. `--from` acepta directorio
o archivo SQLite. Importa todas las muestras al perfil `default`, con tiempos
originales y nulls normalizados. Es idempotente por ruta canónica/proveedor.
La última muestra histórica se utiliza únicamente si el perfil nuevo no tiene
lecturas. No cambia una cuota histórica a 100 % por haber vencido.

SQLite **nunca abre las bases originales**: se copian DB y WAL a un directorio
privado nuevo, se verifica que el origen no cambió durante la copia y se ejecuta
integrity_check. Si detecta cambios, aborta y pide reintentar. No copia .git,
dependencias, target, entornos, credenciales ni cachés originales completas.

El registro histórico de notificaciones original permanece en su SQLite; no se
importa como historial de eventos. La nueva deduplicación de alertas empieza con
estado propio, por lo que puede emitir un aviso inicial si una cuota ya está baja.
Este límite está documentado para no fingir continuidad de avisos que no se migró.

## Sustitución del servicio: decisión del usuario

Primero guardar referencias a tus unidades y overrides, sin borrar nada. Después,
en el momento elegido, podés ejecutar:

```bash
systemctl --user disable --now codex-quota-monitor.service opencode-go-quota-monitor.service
systemctl --user daemon-reload
systemctl --user enable --now ai-quota-monitor.service
```

Así evitás el polling permanente duplicado. Estos comandos **no fueron ejecutados**
por el agente. Las extensiones antiguas pueden deshabilitarse manualmente después
de verificar la nueva. No borrar las carpetas de los proyectos originales.

Rollback, conservando todos los datos:

```bash
systemctl --user disable --now ai-quota-monitor.service
systemctl --user enable --now codex-quota-monitor.service opencode-go-quota-monitor.service
```

## Capacidad preservada

Dos fuentes reales, ventanas 5h/7d/mensual, datos obsoletos, historial persistente,
CSV, alertas 20/10/5, GNOME con detalle y enlaces, servicio de usuario y consulta
de diagnóstico. Las fechas de consola se expresan en Unix UTC; las interfaces
visuales presentan fechas locales. El modo antiguo `--check` se sustituye por
`refresh` aislado mediante `--state-dir` y `--cache`: conserva evidencia del intento
en el destino elegido y no emite alertas si `notifications` está desactivado.
