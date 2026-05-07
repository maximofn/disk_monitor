# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Comandos

Toolchain anclado a `stable` por `rust-toolchain.toml`. Todo se opera desde la raíz del workspace.

```bash
cargo build --workspace                      # debug build de los 3 crates
cargo build --release --workspace            # release (lo que se distribuye)
cargo test --workspace                       # todos los tests
cargo clippy --workspace -- -D warnings      # CI lo exige limpio
cargo fmt --all                              # formateo

# Ejecución manual (para iterar):
./target/release/disk-monitord --bind 127.0.0.1 --port 9126 --sample-interval-ms 1000
./target/release/disk-monitor-tray --backend-url http://127.0.0.1:9126

# Sin /proc real (CI, dev en otra máquina):
./target/release/disk-monitord --mock

# Volcar el icono renderizado a un PNG y salir, sin tocar el panel.
# Imprescindible para depurar fallos visuales sin pelearte con GNOME:
./target/release/disk-monitor-tray --backend-url http://127.0.0.1:9126 --dump-icon /tmp/icon.png
```

`disk_monitor.py` (legacy) sigue funcional y puede correr en paralelo a la versión Rust mientras dure la migración. Usa puerto distinto si arrancas ambos.

## Arquitectura

Workspace Cargo con tres crates:

```
crates/disk-monitor-core    →  tipos compartidos (serde Snapshot/Mount/Usage)
crates/disk-monitord        →  daemon HTTP+SSE que lee /proc/mounts + statvfs
crates/disk-monitor-tray    →  frontend Linux (system tray)
```

El protocolo de datos es REST + Server-Sent Events sobre HTTP. La razón del split: permite que un frontend remoto (Mac/Windows/web, en planificación) consuma las mismas métricas. El backend está pensado para correr 24/7 mientras los frontends locales o remotos van y vienen.

Los tres monitores hermanos (`gpu_monitor`, `cpu_monitor`, `ram_monitor`) son repositorios independientes deployables por separado. Cada uno usa su propio puerto: gpu=9123, cpu=9124, ram=9125, **disk=9126**.

### Flujo del backend (`disk-monitord`)

`main.rs` arranca y mantiene un único `DiskSource` (trait): `ProcfsSource` en producción (lee `/proc/mounts` + `statvfs()` por mount point), `MockSource` cuando se pasa `--mock`.

`sampler::spawn` lanza una task de tokio que muestrea cada N ms y publica en un `tokio::sync::watch::Sender<Snapshot>`. Todos los handlers HTTP leen del `Receiver` (`borrow().clone()`), latencia O(µs). El handler SSE reenvía el watch como stream con `WatchStream`. **No hagas trabajo de I/O desde un handler HTTP** — siempre desde el sampler.

Filtrado de mounts: pseudo-filesystems (`tmpfs`, `proc`, `sysfs`, `cgroup*`, `overlay`, `squashfs`, etc.) y mounts bajo `/snap/`, `/proc/`, `/sys/`, `/dev/`, `/run/` se descartan. Es lo que `df -h` haría sin flags raros.

Cálculo de uso: `total = blocks * frsize`, `free = blocks_available * frsize` (el espacio que el usuario sin privilegios puede consumir, equivalente a `df` y a `shutil.disk_usage` en Python), `used = total - free`. **Cuidado**: `f_frsize` (fragment size), no `f_bsize` — son distintos en algunos FS.

`with_graceful_shutdown` de axum **no se usa** porque espera a que se vacíen las conexiones, y los streams SSE son por naturaleza infinitos: `systemctl stop` quedaría colgado. La salida se hace con `tokio::select!` entre `axum::serve` y la señal.

### Flujo del frontend (`disk-monitor-tray`)

`client::spawn` mantiene un loop de SSE con backoff (1s → 2s → 4s → 5s tope) que se resetea al recibir `Event::Open`. Publica `Update::Connected(snapshot)` o `Update::Disconnected(error)` por mpsc.

El loop principal en `main.rs` consume el mpsc y hace `handle.update(|tray| tray.set_state(...))` sobre el `ksni::TrayService`.

### Acciones por archivo en el menú (`tray.rs` + `actions.rs`)

Bajo cada mount, el sub-submenú "Largest files" lista los top-N archivos. Cada archivo aparece como **3 entradas planas** (no como SubMenu anidado):

```
12.3 GiB  /full/path/to/file       (disabled — solo display)
      ↳ Open in file manager       (Standard, click → nautilus --select)
      ↳ Move to trash…             (Standard, click → zenity confirm + trash)
─────
9.8 GiB  /next/file
      ↳ Open in file manager
      ↳ Move to trash…
```

**Por qué plano y no `MenuItem::SubMenu` por archivo**: la extensión `ubuntu-appindicators` de GNOME no renderiza acciones a 3+ niveles de profundidad — Mount → "Largest files" → archivo (sub-submenú) → acciones se queda silenciosamente sin renderizar. Si en el futuro alguien intenta "limpiar" reagrupando como SubMenu, el menú compila y se muestra pero los items del 3er nivel no aparecen. **Verifica visualmente cualquier cambio a la estructura del menú clickando en GNOME real, no solo con tests.**

Las acciones (`Open in file manager`, `Move to trash`) corren en `std::thread::spawn`-eados desde el closure de ksni. No bloquees el thread de ksni; `zenity` y `nautilus` son procesos hijos que tardan varios cientos de ms.

#### Confirmación + papelera + rescan

`actions::confirm()` lanza `zenity --question`. Si zenity falla o no está instalado, **devuelve `false`** (no preguntes y borres). Borrado destructivo silencioso es la peor UX posible — preferimos refusar antes que sorprender.

`actions::move_to_trash()` usa el crate `trash` (libxdg-trash spec, mismo destino que nautilus → `~/.local/share/Trash/`). Tras un trash exitoso, dispara `actions::trigger_rescan()` que hace `POST /v1/rescan/{mount}` al daemon. El scanner del daemon hace coalesce de bursts (si trasheas 5 archivos seguidos, solo se hace 1 walk por mount).

Si `trash::delete` falla (típicamente PermissionDenied porque el archivo es de root), salta otro `zenity --error` con el mensaje real. Nunca silencioso en una acción destructiva.

### Variables de entorno gráfico (DISPLAY/WAYLAND)

El tray necesita en su environ:
- `DBUS_SESSION_BUS_ADDRESS` y `XDG_RUNTIME_DIR` — para que el icono SNI aparezca en el panel.
- **`DISPLAY` (X11) o `WAYLAND_DISPLAY` (Wayland)** + `XAUTHORITY` — para que `nautilus`, `zenity` y otros hijos puedan abrir ventana.

Cuando se lanza vía `.desktop` autostart en una sesión gráfica normal, las hereda automáticamente. **Cuando lo lanzas a mano desde un shell sin sesión gráfica (ej. ssh, claude shell), las acciones del menú "no hacen nada" porque los hijos no encuentran display.** Síntoma: el closure se dispara y se ve en el log, pero nautilus arranca y termina sin pintar ventana.

Workaround para dev/debug:
```bash
DISPLAY=:0 XAUTHORITY=/run/user/$(id -u)/gdm/Xauthority ./target/release/disk-monitor-tray
```
o más portable, copiar el environ de gnome-shell:
```bash
GNOME_PID=$(pgrep -x gnome-shell | head -1)
env $(grep -z -E "^(DISPLAY|WAYLAND_DISPLAY|XAUTHORITY)=" /proc/$GNOME_PID/environ | xargs -0) ./target/release/disk-monitor-tray
```

`tray::DiskTray::set_state` llama a `refresh_icon_file`: rerenderiza el PNG con `IconRenderer::render_png`, lo escribe en `~/.cache/disk-monitor/icons/disk-monitor-tray-N.png` (donde N es un contador que solo crece), y borra el frame anterior. La impl `Tray` publica `IconName = disk-monitor-tray-N` y `IconThemePath = ~/.cache/disk-monitor/icons`.

#### Punto crítico: por qué un PNG en disco y no `IconPixmap`

SNI permite mandar el icono como bytes ARGB inline. **No lo hagas** — la extensión `ubuntu-appindicators` de GNOME comprime los pixmaps anchos a una proporción cuadrada y mangle el icono multi-mount. La estrategia de archivo + `IconName` con contador incremental es exactamente lo que `AppIndicator3.set_icon_full(path, ...)` hace internamente y es la única que GNOME respeta a anchura nativa.

Si alguien "limpia" esto sustituyéndolo por `icon_pixmap()` se rompe visualmente en GNOME aunque pase los tests.

### Render del icono (`icon::render`)

`tiny-skia` produce RGBA **premultiplicado**. Hay dos rutas de salida:

- `unpremultiply_to_rgba` → para el PNG del disco (los visores PNG asumen straight RGBA).
- `rgba_premul_to_argb_straight` → para `IconPixmap` por si en algún momento se vuelve a usar.

Texto: `freetype-rs` con DejaVu Sans Mono **Regular** y tamaño `0.45 * h` redondeado a entero. `freetype` ejecuta el TrueType bytecode interpreter completo, que es lo que mantiene los strokes finos pixel-aligned y nítidos a 10 px (mismo motor que PIL/freetype del tray Python). Versiones previas con `fontdue` o `ab_glyph` rasterizaban borroso a 10–12 px. Si vuelves a uno de esos rasterizadores se nota visualmente — verifícalo con `--dump-icon` antes de mergear.

Layout per-mount (con `icon_height = 22` px):

```
[disk icon ~22] 2px [label] 2px [donut 18x18]   <gap 4px>   [siguiente mount...]
```

- Label: short leaf del mount point (`/` → `/`, `/home` → `home`, `/media/wallabot/seagate2T` → `seagat`), truncado a 6 chars.
- El ancho del label se calcula como el máximo entre todos los mounts, así los donuts no saltan de posición frame a frame si un nombre es más corto que otro.

**Paleta** (constantes `[u8; 4]` RGBA en `render.rs`):

| const            | hex       | uso                                         |
|---               |---        |---                                          |
| `COLOR_TEXT`     | `#ffffff` | label normal, número dentro del donut       |
| `COLOR_FREE`     | `#66b3ff` | anillo: porción libre                       |
| `COLOR_OK`       | `#99ff99` | anillo lleno <70%                           |
| `COLOR_WARN1`    | `#ffdb4d` | anillo 70–80%                               |
| `COLOR_WARN2`    | `#ffcc99` | anillo 80–90%                               |
| `COLOR_HIGH`     | `#ff6666` | anillo ≥90%                                 |

**Estado disconnected**: todo el bloque pasa a gris `#aaaaaa` (texto), `#808080` (anillo libre), `#606060` (anillo usado).

**Único umbral**: porcentaje de uso del filesystem (no hay otra métrica como temperatura del GPU). Por eso label y número del centro van neutros — el wedge del donut ya hace el code de color. Si pintas el número en rojo cuando el disco está al 95%, encima de un anillo ya rojo, queda redundante y ruidoso.

## Home Assistant (`home-assistant/`)

Integración declarativa con HA usando el componente `rest` de `default_config`. 18 sensores: host/mount_count + 8 por mount hardcodeado (`/` y `/media/wallabot/seagate2T`).

**Topología**: túnel SSH forward desde raspihome (always-on) al host con disco, puerto 9126. Pubkey en wallabot con `restrict,port-forwarding,permitopen="127.0.0.1:9126"`.

**`scan_interval: 60s`** (no 15s como CPU/GPU/RAM). El uso de disco no cambia segundo a segundo y los `largest_files` se refrescan en background mucho más despacio. History-graphs en el dashboard a 72 horas (no 6h) por la misma razón — el disco evoluciona en escala de días.

**Mount lookup por `selectattr`, no por índice**:

```yaml
{% set m = value_json.mounts | selectattr('mount_point', 'eq', '/') | first | default(none) %}
```

El orden del array `mounts[]` que devuelve el daemon no está garantizado entre samples (depende del orden en `/proc/mounts` que puede cambiar tras un remount). `mounts[0]` sería frágil; `selectattr` es robusto. Para añadir un mount nuevo, copia el bloque entero y cambia el `mount_point` del filtro y los `unique_id`.

**`largest_files` como atributo**, no entidades separadas. Top-N (típicamente 20) archivos con `path` + `size_bytes` van en `attributes.largest_files` del sensor `disk_<slug>_largest_file`. Visible en developer-tools y consumible desde plantillas / scripts. Crear N entidades por archivo inflaría el registro y haría las gráficas de "top-1" inmanejables cuando el archivo más grande cambia entre scans.

**`POST /v1/rescan/{mount}` no expuesto en el package**. El componente `rest` de HA es solo GET. Para disparar rescans desde HA hay que definir `rest_command:` aparte (documentado en `home-assistant/README.md`). La mayoría de usos no lo necesitan — el daemon hace coalesce y los scans se programan solos cada N horas.

**Schema replication**: si añades un campo a `Mount` / `Usage` / `FileEntry` en `disk-monitor-core`, replícalo en `home-assistant/packages/disk_monitor.yaml` como nuevo `value_template`.

## Convenciones del repo

- **API versioning** por prefijo de path (`/v1/...`). Romper compat = subir a `/v2/`. `disk_monitor_core::API_VERSION` es la fuente de verdad.
- **Tipos serializados** viven en `disk-monitor-core`. Si añades un campo a `Snapshot` / `Mount` / `Usage`, tanto backend como tray lo ven sin drift, pero **es un cambio de schema** — clientes externos pueden romperse.
- **Defaults seguros**: el daemon bindea `127.0.0.1` sin auth.
- **Dependencias compartidas** declaradas en `[workspace.dependencies]` del `Cargo.toml` raíz; los crates las referencian con `{ workspace = true }`. Solo añade deps específicas al `Cargo.toml` del crate cuando solo lo use ese crate.
- **Logging** vía `tracing` + `tracing-subscriber` en ambos binarios; controlable con `RUST_LOG` o `--log-level`.
- **Tests del frontend** evitan dependencias de runtime gráfico: render del icono se testa generando pixmaps y comparando bytes, no abriendo ventanas.
