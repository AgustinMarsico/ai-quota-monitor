# Terminal

Todas las integraciones llaman `status --compact`, exclusivamente local. No usar
`refresh` en PS1, precmd o Starship. Los porcentajes son restantes; `~` es obsoleto.
`--color never` evita secuencias ANSI que alteren el ancho del prompt.

## Starship

Merge manual de [`integrations/shell/starship.toml`](../integrations/shell/starship.toml).
Si tu `format` principal es explícito, agregá `${custom.ai_quota}` donde prefieras.
El formato predeterminado `$all` incluye módulos custom. El comando usa Bash sin
archivos de inicio. Configurá `command_timeout` si tu máquina lo requiere;
la medición local está en el informe de validación.

## Bash

Agregar manualmente, luego de definir PS1:

```bash
source /ruta/ai-quota-monitor/integrations/shell/bash.sh
```

El fragmento antepone una línea al PS1 existente y evita carga doble. No reemplaza
PROMPT_COMMAND ni ejecuta adquisiciones de datos. No usamos colores ANSI incrustados
en sustituciones porque necesitan contabilidad especial de caracteres invisibles.

## Zsh

```zsh
source /ruta/ai-quota-monitor/integrations/shell/zsh.zsh
```

Activa `prompt_subst` y antepone la salida local al PROMPT existente. Revisá esta
opción si tu tema ya administra la composición del prompt.

## Configurar

`compact_metric: "remaining"` es el default. Para mostrar uso cambiar a `"used"`
o usar `--metric used`; en ese caso recordá rotular tu prompt como usado.
`compact_color` acepta `never`, `auto` o `always`; `NO_COLOR` tiene prioridad.
Las interfaces visuales siempre muestran restante para mantener una lectura coherente.

Podés filtrar `--provider codex` o `--provider opencode-go`. Los perfiles adicionales
se rotulan entre corchetes. Una caché ausente produce `?`; la CLI no dispara red.
