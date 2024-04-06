#!/usr/bin/env python3
import signal
import gi
gi.require_version('AppIndicator3', '0.1')
from gi.repository import AppIndicator3, GLib
from gi.repository import Gtk as gtk
import os
import subprocess
import webbrowser
import shutil

APPINDICATOR_ID = 'GPU_monitor'

def main():
    path = os.path.dirname(os.path.realpath(__file__))
    icon_path = os.path.abspath(f"{path}/disk.png")
    Disk_indicator = AppIndicator3.Indicator.new(APPINDICATOR_ID, icon_path, AppIndicator3.IndicatorCategory.SYSTEM_SERVICES)
    Disk_indicator.set_status(AppIndicator3.IndicatorStatus.ACTIVE)
    Disk_indicator.set_menu(build_menu())

    # Get disk info
    GLib.timeout_add_seconds(1, update_disk_info, Disk_indicator)

    GLib.MainLoop().run()

def open_repo_link(_):
    webbrowser.open('https://github.com/maximofn/disk_monitor')

def buy_me_a_coffe(_):
    webbrowser.open('https://www.buymeacoffee.com/maximofn')

def build_menu():
    menu = gtk.Menu()

    memory_info = get_disk_info()

    memory_free = gtk.MenuItem(label=f"Free: {memory_info['free']:.2f} GB")
    menu.append(memory_free)

    memory_used = gtk.MenuItem(label=f"Used: {memory_info['used']:.2f} GB")
    menu.append(memory_used)

    memory_total = gtk.MenuItem(label=f"Total: {memory_info['total']:.2f} GB")
    menu.append(memory_total)

    horizontal_separator1 = gtk.SeparatorMenuItem()
    menu.append(horizontal_separator1)

    item_repo = gtk.MenuItem(label='Repository')
    item_repo.connect('activate', open_repo_link)
    menu.append(item_repo)

    item_buy_me_a_coffe = gtk.MenuItem(label='Buy me a coffe')
    item_buy_me_a_coffe.connect('activate', buy_me_a_coffe)
    menu.append(item_buy_me_a_coffe)

    horizontal_separator2 = gtk.SeparatorMenuItem()
    menu.append(horizontal_separator2)

    item_quit = gtk.MenuItem(label='Quit')
    item_quit.connect('activate', quit)
    menu.append(item_quit)

    menu.show_all()
    return menu

def update_disk_info(indicator):
    disk_info = get_disk_info()

    info = f"Free:{disk_info['free']:.2f}GB/Used:{disk_info['used']:.2f}GB/Total:{disk_info['total']:.2f}GB"
    indicator.set_label(info, "Indicator")

    return True

def get_disk_info():
    # Get disk usage
    total, used, free = shutil.disk_usage("/")

    # Transform bytes to GB
    total_gb = total / 1024**3
    free_gb = free / 1024**3
    used_gb = used / 1024**3

    return {"total": total_gb, "free": free_gb, "used": used_gb}

if __name__ == "__main__":
    signal.signal(signal.SIGINT, signal.SIG_DFL) # Allow the program to be terminated with Ctrl+C
    main()
