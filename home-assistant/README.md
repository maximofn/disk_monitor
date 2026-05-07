# Home Assistant integration

Expone el estado de los discos (`disk-monitord`, puerto 9126) en Home
Assistant como sensores nativos. Sin custom component: solo configuración
YAML usando la integración `rest` que viene con `default_config`.

## Arquitectura

```
[ Ubuntu (wallabot) ]                 [ Raspberry (raspihome) ]
  disk-monitord                           Home Assistant (Docker, host net)
  127.0.0.1:9126  ◄──── ssh -L ─────  127.0.0.1:9126
                                            │
                                            └─► sensor.rest (scan_interval=60s)
```

`scan_interval=60s` (no 15s como CPU/GPU/RAM): el uso de disco no cambia
segundo a segundo y los `largest_files` se refrescan en background mucho
más despacio. Si quieres ver cambios rápidos por copia/borrado puntual,
forza un `homeassistant.update_entity` sobre el sensor relevante o baja el
intervalo.

## Mounts hardcodeados

El paquete tiene **dos bloques** hardcodeados por mount point:

- `/` (root) → entidades `sensor.disk_root_*`
- `/media/wallabot/seagate2T` → entidades `sensor.disk_seagate2t_*`

Resolución por `selectattr` sobre `mount_point` (no por índice en el array
`mounts[]`), así que el orden que devuelva el daemon no importa.

Para añadir otro mount: copia un bloque entero, sustituye el `mount_point`
del `selectattr` y los `unique_id` de `disk_<slug>_*` (slug = identificador
sin barras ni espacios). Replica también en
`home-assistant/lovelace/disk_dashboard.yaml`.

## Instalación

### 1) Túnel SSH desde raspihome

```bash
# En raspihome (linger ya habilitado si desplegaste otro monitor antes):
cd /ruta/al/repo/disk_monitor/home-assistant/tunnel
./install.sh
```

Genera `~/.ssh/id_ed25519_disk_tunnel`, imprime la línea para
`authorized_keys` de wallabot
(`restrict,port-forwarding,permitopen="127.0.0.1:9126" ssh-ed25519 ...`),
instala `disk-monitor-ha-tunnel.service`.

Verifica:

```bash
systemctl --user status disk-monitor-ha-tunnel.service
curl -fsS http://127.0.0.1:9126/v1/info | jq
```

### 2) Paquete de Home Assistant

```bash
scp packages/disk_monitor.yaml raspihome:/home/raspihome/docker/homeassistant/packages/
ssh raspihome 'docker restart homeassistant'
```

Si no tienes `homeassistant: { packages: !include_dir_named packages }` en
`/config/configuration.yaml`, añádelo una vez antes del restart.

Tras recargar, en HA aparecen las entidades:

```
sensor.disk_monitor_host           sensor.disk_root_*           sensor.disk_seagate2t_*
sensor.disk_monitor_mount_count    (device, fs, total, used,    (mismo set)
                                    free, used_percent,
                                    largest_file,
                                    largest_file_size)
```

`sensor.disk_*_largest_file` lleva como atributos `largest_files` (lista
completa de top-N archivos con `path` y `size_bytes`) y
`largest_files_scanned_at` (timestamp del último escaneo).

### 3) Dashboard (opcional)

`lovelace/disk_dashboard.yaml` — pegar como vista nueva en el Raw editor.
History-graph configurado a **72 horas** (vs. 6h para CPU/GPU/RAM) porque
el disco evoluciona en escala de días, no minutos.

## Endpoint mutador `/v1/rescan` — no expuesto en HA

`disk-monitord` ofrece `POST /v1/rescan/{mount}` para forzar un walk de los
archivos más grandes. El componente `rest` de HA no soporta POST con
parámetros dinámicos sobre el path; si quieres dispararlo desde HA, define
un `rest_command:` aparte:

```yaml
rest_command:
  disk_rescan_root:
    url: http://127.0.0.1:9126/v1/rescan//
    method: POST
```

Y un `script:` o automatización que lo llame. Fuera del scope del paquete
base — la mayoría de usos no lo necesitan, el daemon hace coalesce
internamente y los scans se programan solos cada N horas.

## Troubleshooting

- **Sensores `unavailable`**: `systemctl --user status disk-monitor-ha-tunnel.service` y `curl http://127.0.0.1:9126/healthz`.
- **`sensor.disk_root_largest_file` queda `none`**: el scan en background aún no terminó, o se desactivó con `--no-largest-files` en el daemon.
- **`administratively prohibited`** en logs del túnel: línea en `authorized_keys` sin `port-forwarding`. Debe ser `restrict,port-forwarding,permitopen="127.0.0.1:9126" ...`.
