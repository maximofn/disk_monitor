#!/usr/bin/env python3
import signal
import gi
gi.require_version('AppIndicator3', '0.1')
from gi.repository import AppIndicator3, GLib
from gi.repository import Gtk as gtk
import os
import webbrowser
import shutil
import matplotlib.pyplot as plt
from PIL import Image

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
    _ = get_disk_info()

    # info = f"Free:{disk_info['free']:.2f}GB/Used:{disk_info['used']:.2f}GB/Total:{disk_info['total']:.2f}GB"
    # indicator.set_label(info, "Indicator")
    
    # Show pie chart
    path = os.path.dirname(os.path.realpath(__file__))
    icon_path = os.path.abspath(f"{path}/disk_info.png")
    indicator.set_icon_full(icon_path, "RAM Usage")

    return True

def get_disk_info():
    # Get disk usage
    total, used, free = shutil.disk_usage("/")

    # Transform bytes to GB
    total_gb = total / 1024**3
    free_gb = free / 1024**3
    used_gb = used / 1024**3

    blue_color = '#66b3ff'
    red_color = '#ff6666'
    green_color = '#99ff99'
    orange_color = '#ffcc99'
    yellow_color = '#ffdb4d'
    percentage_warning1 = 70
    percentage_warning2 = 80
    percentage_caution = 90

    # Create pie chart
    labels = 'Used', 'Free'
    sizes = [used_gb / total_gb * 100, free_gb / total_gb * 100]  # Percentage of used and free memory
    percentage_of_use = sizes[0]

    if percentage_of_use < percentage_warning1:
        used_color = green_color
    elif percentage_of_use >= percentage_warning1 and percentage_of_use < percentage_warning2:
        used_color = yellow_color
    elif percentage_of_use >= percentage_warning2 and percentage_of_use < percentage_caution:
        used_color = orange_color
    else:
        used_color = red_color
    total_color = blue_color
    colors = [used_color, total_color]
    explode = (0.1, 0)  # Explode used memory

    fig, ax = plt.subplots()
    ax.pie(sizes, explode=explode, labels=labels, colors=colors, autopct='%1.1f%%',
           startangle=90, pctdistance=0.85, counterclock=False, wedgeprops=dict(width=0.3, edgecolor='w'))

    # Draw a circle at the center of pie to make it look like a donut
    centre_circle = plt.Circle((0,0),0.70,fc='none', edgecolor='none')
    fig = plt.gcf()
    fig.gca().add_artist(centre_circle)

    ax.axis('equal')  # Mantain the aspect ratio
    plt.tight_layout()

    # Save pie chart
    path = os.path.dirname(os.path.realpath(__file__))
    plt.savefig(f'{path}/disk_chart.png', transparent=True)
    plt.close(fig)

    # Define icon height
    icon_height = 22
    padding = 10

    # Load íconos
    disk_icon = Image.open(f'{path}/disk.png')
    disk_chart = Image.open(f'{path}/disk_chart.png')

    # Resize icons
    disk_icon_relation = disk_icon.width / disk_icon.height
    disk_icon_width = int(icon_height * disk_icon_relation)
    scaled_disk_icon = disk_icon.resize((disk_icon_width, icon_height), Image.LANCZOS)

    # Resize chart
    chart_icon_relation = disk_chart.width / disk_chart.height
    chart_icon_width = int(icon_height * chart_icon_relation)
    scaled_disk_chart = disk_chart.resize((chart_icon_width, icon_height), Image.LANCZOS)

    # New image with the combined icons
    total_width = scaled_disk_icon.width + scaled_disk_chart.width
    combined_image = Image.new('RGBA', (total_width, icon_height+padding), (0, 0, 0, 0))  # Fondo transparente

    # Combine icons
    combined_image.paste(scaled_disk_icon, (0, int(padding/2)), scaled_disk_icon)
    combined_image.paste(scaled_disk_chart, (scaled_disk_icon.width, int(padding/2)), scaled_disk_chart)

    # Save combined image
    combined_image.save(f'{path}/disk_info.png')

    return {"total": total_gb, "free": free_gb, "used": used_gb}

if __name__ == "__main__":
    path = os.path.dirname(os.path.realpath(__file__))
    if os.path.exists(f'{path}/disk_info.png'):
        os.remove(f'{path}/disk_info.png')
    signal.signal(signal.SIGINT, signal.SIG_DFL) # Allow the program to be terminated with Ctrl+C
    main()
