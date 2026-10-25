# Extensión GNOME: vista compacta y configuración

La extensión lee únicamente `$XDG_CACHE_HOME/ai-quota/status.json` (normalmente
`~/.cache/ai-quota/status.json`). No ejecuta la CLI ni consulta APIs. Relee cada
30 segundos y al abrir el menú; recalcula frescura y cuenta regresiva con el reloj
actual. Los cambios de porcentajes actualizan los actores existentes, conservando
el menú abierto. Los detalles se actualizan mientras su submenú esté desplegado.

## Modos de barra superior

Seleccionar **Modo visual** en el propio menú. Se guarda inmediatamente con
GSettings y se conserva entre sesiones:

| Modo | Ejemplo |
|---|---|
| **Compacto**, predeterminado | `Codex 7d 77% · Go 7d 85%` |
| **Mínimo** | `C 77% · G 85%` |
| **Completo** | `Codex 5h 90% / 7d 77% · Go 5h 95% / 7d 85% / m 99%` |

Compacto y mínimo seleccionan el menor porcentaje restante **válido**, sin
redondear antes de comparar. Los empates conservan la primera ventana de la caché.
`0%` es un valor válido; no se interpreta como ausencia. Cada perfil adicional se
muestra por separado con su nombre entre corchetes. El modo completo agrupa las
ventanas bajo el nombre del proveedor, sin repetirlo.

- `?`: sin porcentaje disponible. En el menú, **No disponible**, sin barra.
- `~`: datos obsoletos: edad superior a `stale_seconds`, estado `stale`, fecha
  futura inconsistente, última consulta ausente o restablecimiento ya vencido.
  También se controla la edad de la caché completa. No se supone una renovación.
- `!`: consulta fallida. Si existen porcentajes previos, se conservan marcados.
- **Advertencia**: menos del 20% restante, color de advertencia en el indicador y
  `⚠` junto al porcentaje de la ventana. Se evalúa el valor original, no redondeado.

Una caché ausente, ilegible, inválida o vacía muestra `AI quota ?`; nunca se rellena
un porcentaje desconocido con 100%. La etiqueta accesible del indicador contiene
el proveedor, todos los períodos, porcentajes y estado incluso en modo mínimo.

## Menú

Dos encabezados, **Codex** y **OpenCode Go**, y cinco filas: 5 horas y 7 días para
Codex; 5 horas, 7 días y Mensual para Go. Cada fila muestra porcentaje restante y
una barra de nivel nativa de GNOME Shell, de solo lectura. Las ventanas ausentes
se mantienen como **No disponible**. Se conservan otras ventanas que aparezcan en
la caché y perfiles adicionales.

**Detalles avanzados**, cerrado inicialmente, muestra por proveedor:

- Fuente, identificador de proveedor y perfil.
- Última consulta exitosa, en fecha/hora local, o **No informada**.
- Estado de frescura y error de consulta cuando exista.
- Fecha/hora local y cuenta regresiva de cada restablecimiento.

Sin `resets_at`, se muestra **No informado por la fuente**, también para Go.
Las fechas vencidas se identifican como tales. No se calculan fechas a partir de
la duración de las ventanas. Se usa desplazamiento nativo del submenú y texto
avanzado ajustado a varias líneas; los nombres largos se acotan visualmente.
Se conservan **ChatGPT Usage** y **OpenCode Console**.

## Actualizar únicamente la extensión

Desde la raíz del repositorio:

```bash
python3 scripts/install_gnome.py
```

El destino predeterminado es
`$XDG_DATA_HOME/gnome-shell/extensions/ai-quota@local`, o
`~/.local/share/gnome-shell/extensions/ai-quota@local`. Copia la extensión, el
validador compartido y compila su esquema local con `glib-compile-schemas`.
No requiere compilar Rust. No modifica binario, unidades systemd, configuración
del monitor, SQLite, caché, credenciales ni archivos de OpenCode.

En **GNOME/Wayland**, cerrar sesión y volver a entrar para cargar el JavaScript
actualizado. No hace falta reiniciar el servicio del monitor. Desactivar/activar
la extensión no garantiza invalidar los módulos JavaScript ya cargados en Shell.
Si aún no estaba habilitada, después de entrar:

```bash
gnome-extensions enable ai-quota@local
gnome-extensions info ai-quota@local
```

Para cambiar el modo por terminal, después de instalar:

```bash
EXT="${XDG_DATA_HOME:-$HOME/.local/share}/gnome-shell/extensions/ai-quota@local"
gsettings --schemadir "$EXT/schemas" set org.gnome.shell.extensions.ai-quota display-mode 'minimal'
# Valores: compact, minimal, full. Restablecer el predeterminado:
gsettings --schemadir "$EXT/schemas" reset org.gnome.shell.extensions.ai-quota display-mode
```

## Validación aislada

```bash
node tests/gnome.mjs
python3 scripts/install_gnome.py --destination target/gnome-test/ai-quota@local
env GSETTINGS_BACKEND=memory \
  GI_TYPELIB_PATH=/usr/lib64/gnome-shell:/usr/lib64/mutter-18 \
  LD_LIBRARY_PATH=/usr/lib64/gnome-shell:/usr/lib64/mutter-18 \
  gjs -m tests/gjs-gnome.mjs target/gnome-test/ai-quota@local/schemas
```

Las rutas de Mutter corresponden a Fedora con GNOME Shell 50; comprobadas
inicialmente con **50.5** y en esta auditoría con **50.6**. La prueba nativa
carga la clase real `BarLevel` y verifica las APIs de menú/desplazamiento, los
modos permitidos por GSettings y
la presentación en GJS. Usa un backend de configuración en memoria y no actúa
sobre la sesión. Las pruebas Node cubren selección/cambio de mínimo, cero,
ausencias, errores, obsolescencia, los tres modos, estructura del menú, barras,
advertencias y lifecycle, incluida una lectura pendiente al deshabilitar.

La apariencia final, escalado y tema se comprueban al cargar la extensión en la
sesión real; las verificaciones aisladas no sustituyen esa inspección visual.
