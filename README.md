# Cpu monitor

Monitor of disk storage for Ubuntu menu bar

![disk monitor](disk_monitor.gif)

## Installation

```bash
sudo apt install lm-sensors
```

Select YES to all questions

```bash
sudo sensors-detect
```

Install psensor

```bash
sudo apt install psensor
```

## Add to startup

```bash
disk_monitor.sh
```