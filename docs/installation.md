# Instalación Fedora y systemd de usuario

## Preparar

```bash
cargo build --release --locked
python3 scripts/install.py
```

El script copia el binario a `~/.local/bin`, el plugin y schema a
`~/.local/share/ai-quota`, configuración a `~/.config/ai-quota`, y unidad a
`~/.config/systemd/user`. Conserva un config existente. No ejecuta systemctl,
no cambia OpenCode y no habilita GNOME. Registra un manifiesto de archivos.

La unidad referencia el binario instalado, no el directorio fuente. El instalador
localiza Codex en el PATH y guarda su ruta en el config nuevo. Si usás un wrapper
con `/usr/bin/env node`, asegurá que Node esté en el PATH del servicio o usá el
ejecutable nativo de Codex. `doctor` no necesita conexión externa.

Instalación de prueba completamente aislada:

```bash
python3 scripts/install.py --prefix "$PWD/sandbox/install" \
  --config-dir "$PWD/sandbox/install/config" --unit-dir "$PWD/sandbox/install/units" \
  --gnome-dir "$PWD/sandbox/install/gnome"
systemd-analyze --user verify sandbox/install/units/ai-quota-monitor.service
```

## Activación explícita, después de decidir la migración

Estos comandos afectan la sesión real; no fueron ejecutados en la entrega:

```bash
systemctl --user daemon-reload
systemctl --user enable --now ai-quota-monitor.service
systemctl --user status ai-quota-monitor.service
journalctl --user -u ai-quota-monitor.service -n 40 --no-pager
ai-quota doctor
```

Parar/reiniciar:

```bash
systemctl --user stop ai-quota-monitor.service
systemctl --user restart ai-quota-monitor.service
```

El servicio usa `Restart=on-failure`, `RestartSec=20`, `UMask=0077`,
`NoNewPrivileges=yes` y `TimeoutStopSec=60`. Un fallo remoto genera backoff,
no un reinicio del proceso. Un fallo de disco se registra y se reintenta.
La parada espera que termine el transporte acotado. No usa privilegios elevados.
Cambios de config requieren reiniciar el daemon; no se recargan silenciosamente.

No hace falta `enable-linger` para uso durante tus sesiones. Habilitarlo mantendría
el manager y el monitor activos sin sesión, agregando consultas y consumo; evaluarlo
solo si ese funcionamiento es deseado. No se cambia automáticamente.

## GNOME opcional

```bash
python3 scripts/install.py --gnome-dir "$HOME/.local/share/gnome-shell/extensions"
# Después de que GNOME reconozca la extensión (puede requerir nueva sesión):
gnome-extensions enable ai-quota@local
```

La nueva extensión reemplaza los dos indicadores con una sola lectura de la
caché, detalle, reinicios y enlaces a ambas páginas. Los originales no se
deshabilitan automáticamente. Metadata GNOME 48–51; validación visual real pendiente.

## Actualizar y desinstalar

Compilar de nuevo y ejecutar el mismo instalador actualiza archivos sin tocar
configuración/historial. Reiniciar el servicio explícitamente para usar el nuevo binario.

```bash
systemctl --user disable --now ai-quota-monitor.service
gnome-extensions disable ai-quota@local  # si se habilitó
python3 scripts/uninstall.py
systemctl --user daemon-reload
```

La desinstalación rechaza un servicio nuevo activo y elimina únicamente archivos
del manifiesto cuyo contenido no fue modificado. Conserva configuración, SQLite,
caché y credenciales. Retirá manualmente la referencia del plugin de OpenCode y
reiniciá OpenCode. Nunca elimina los monitores originales.
