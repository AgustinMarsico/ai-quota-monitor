# OpenCode: plugin TUI público

Probado con **OpenCode 1.18.35**. La API pública `@opencode-ai/plugin/tui`
expone slots, keymap, diálogos y lifecycle. Se verificaron primero las declaraciones
instaladas y el código del tag; después una prueba mínima real con un toast.
El módulo debe exportar por defecto `{id, tui}`; exportar solo una función nombrada
no cargó el plugin en esa versión.

El plugin completo agrega:
- Barra persistente en el slot público `app_bottom`.
- `/quota` y "AI quota: show details" en la paleta.
- Detalle de ventanas, porcentajes restantes, reinicios, último éxito y fuente.
- Lectura de caché cada cinco segundos, liberada al descargar el plugin.

No tiene adaptadores, credenciales ni HTTP. La lectura visual periódica no constituye
un segundo sistema de adquisición. No ejecuta un LLM para consultar las cuotas.

## Activar manualmente

Después de instalar archivos, agregar a **tui.json**, no a opencode.json:

```json
{
  "$schema": "https://opencode.ai/tui.json",
  "plugin": ["file:///home/TU_USUARIO/.local/share/ai-quota/opencode/plugin.tsx"]
}
```

Preservá las entradas existentes y usá tu ruta real. Para una caché diferente:

```json
{
  "$schema": "https://opencode.ai/tui.json",
  "plugin": [["file:///ruta/plugin.tsx", {"cachePath": "/ruta/status.json"}]]
}
```

**Salir y reiniciar OpenCode** para cargar los cambios. No se modificó tu configuración
actual. El plugin utiliza el Solid/OpenTUI suministrado por el host, mediante sus
aliases de runtime; no requiere instalar Node/npm para usarlo.

## Prueba real aislada

```bash
python3 -m venv target/test-env
target/test-env/bin/pip install -r tests/requirements.txt
target/test-env/bin/python tests/opencode_pty.py --opencode "$(command -v opencode)"
target/test-env/bin/python tests/opencode_pty.py --opencode "$(command -v opencode)" \
  --plugin "$PWD/integrations/opencode/plugin.tsx" --expect 'Codex 5h: 78%' --exercise
```

El harness crea HOME y todos los XDG en un directorio temporal de `target/`.
No hereda claves de proveedor; usa un modelo fixture con destino loopback inactivo,
no envía prompts y no consume cuotas. Reconstruye la pantalla con pyte para comprobar
render inicial, actualización de caché y diálogo `/quota`.

Esta API puede evolucionar: un upgrade de OpenCode requiere repetir typecheck y
la prueba PTY. No depende de parches ni imports de código interno. La barra persistente
puede ocupar espacio adicional en terminales angostas.

Alternativa oficial sin plugin:

```text
!ai-quota status --compact
```

Los comandos personalizados Markdown que inyectan shell output como prompt pueden
enviar esos datos a un LLM; se prefiere el plugin o el comando shell directo.
