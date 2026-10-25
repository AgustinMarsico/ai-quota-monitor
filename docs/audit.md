# Auditoría y seguridad

Fuentes originales: `codex-quota-monitor` y `opencode-go-quota-monitor` en la carpeta
hermana. Ambas Rust 2021 con Serde, serde_json y rusqlite bundled, módulos main,
model, protocol y storage, SQLite/JSON y extensiones GNOME. Licencias MIT;
atribuciones conservadas en LICENSE y NOTICE. Los directorios entregados no eran
repositorios Git. No se modificaron sus archivos.

En la auditoría los servicios estaban activos y habilitados, sin reinicios.
ExecStart apuntaba a `~/.local/bin`; el override Codex a `~/.local/bin/codex`.
No se encontraron rutas de las carpetas fuente en las unidades/overrides inspeccionados.
El nuevo proyecto es independiente de esas fuentes.

Problemas corregidos: duplicación del ciclo, falta de backoff/Retry-After,
obsolescencia en CLI, escritores no coordinados, errores Codex remotos no saneados,
auth Go con ruta XDG fija, fechas ausentes como cero, GJS sin promisificación
explícita y rearmado diario artificial de alertas sin reinicio conocido.

## Autenticación

Codex: protocolo app-server publicado, initialize → initialized →
account/rateLimits/read; no thread/start ni turn/start. No se inicia una conversación.
No se leen ni copian tokens Codex. La CLI upstream puede gestionar su propio
refresh OAuth/logging de acuerdo con su configuración; el monitor no escribe
credenciales ni llama métodos de login/logout.

Go en modo `api_key`: lee auth.json sin modificarlo, o un override de entorno. La clave se valida
antes de enviarse a curl por stdin. No aparece en argv, archivos temporales,
SQLite, JSON, errores o doctor. curl ignora ~/.curlrc, no sigue redirecciones,
acepta solo HTTPS y usa un destino fijo con timeouts y límite de respuesta.
Go en modo `console_oauth` usa credenciales privadas del monitor, nunca el
archivo de OpenCode ni el override de API key. Sus tokens se guardan con permisos
privados y se pasan a curl por stdin; ante un refresh de resultado incierto no
reutiliza el token. Ver [OpenCode Go y Console](opencode-go.md).
La ausencia de credenciales y HTTP 401/403 se distinguen de conectividad y 429.

El endpoint `https://opencode.ai/zen/go/v1/usage` está observado en CodexBar y en
el monitor original, pero su contrato no está documentado en la página oficial
de Go consultada. Puede cambiar o dejar de funcionar sin garantía. No se usan
cookies web ni scraping de consola como fallback. Tampoco se calculan cuotas
a partir de costos locales ni tablas de modelos.

No se imprimen cuerpos remotos ni detalles arbitrarios de JSON-RPC. Los errores
públicos son mensajes cerrados. Datos privados tienen directorios 0700 y archivos
0600; el proceso usa umask0077 y nunca necesita root. El historial puede revelar
patrones de uso: tratarlo como dato privado aunque no contenga claves.

`doctor` es offline: comprueba configuración, caché, ejecutables/archivo de Go y
estado systemd. No verifica que una credencial siga vigente en el servidor;
eso requiere una consulta explícita. El estado del servicio es informativo para
instalaciones sandbox y no implica autorización de activación.

Fuentes:
- https://github.com/openai/codex (app-server)
- https://opencode.ai/docs/go/
- https://github.com/steipete/CodexBar/blob/main/Sources/CodexBarCore/Providers/OpenCodeGo/OpenCodeGoUsageFetcher.swift
- https://github.com/anomalyco/opencode/blob/v1.18.35/packages/plugin/src/tui.ts
- https://github.com/anomalyco/opencode/blob/v1.18.35/packages/tui/src/config/index.tsx

La suite principal es offline y no depende de que esos servicios respondan.
