#!/usr/bin/bash
# Get script path
SCRIPT_PATH=$(dirname $0)
/usr/bin/python3 $SCRIPT_PATH/disk_monitor.py >disk_monitor.log 2>disk_monitor_error.log