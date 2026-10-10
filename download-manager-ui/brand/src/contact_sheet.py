#!/usr/bin/env python3
"""Render review sheets of the built assets. Usage: python3 src/contact_sheet.py dist out_dir"""
import os, sys, json
from PIL import Image, ImageDraw, ImageFont

DIST, OUT = sys.argv[1], sys.argv[2]
os.makedirs(OUT, exist_ok=True)
# IBM Plex Sans Medium (OFL); set PLEX_MEDIUM or put it in src/fonts/
font = ImageFont.truetype(os.environ.get("PLEX_MEDIUM", os.path.join(os.path.dirname(os.path.abspath(__file__)), "fonts", "IBMPlexSans-Medium.otf")), 12)
manifest = json.load(open(f"{DIST}/icons/manifest.json"))

# Icon sheet: each icon at 48 / 24 / 16 on dark, label below
cols, cw, ch = 8, 150, 110
rows = (len(manifest) + cols - 1) // cols
sheet = Image.new("RGB", (cols * cw, rows * ch), "#14171C")
d = ImageDraw.Draw(sheet)
for i, m in enumerate(manifest):
    x, y = (i % cols) * cw, (i // cols) * ch
    big = Image.open(f"{DIST}/icons/png/light/48/{m['id']}.png")
    sheet.paste(big, (x + 14, y + 14), big)
    med = Image.open(f"{DIST}/icons/png/light/24/{m['id']}.png")
    sheet.paste(med, (x + 76, y + 26), med)
    sm = Image.open(f"{DIST}/icons/png/light/16/{m['id']}.png")
    sheet.paste(sm, (x + 112, y + 30), sm)
    d.text((x + 14, y + 74), m["id"], fill="#98A1AD", font=font)
sheet.save(f"{OUT}/icons-sheet.png")

# Brand sheet
B = Image.new("RGB", (1400, 900), "#0D0F12")
L = Image.new("RGB", (700, 900), "#F3F4F6")
B.paste(L, (700, 0))
def put(path, xy, size=None):
    im = Image.open(path).convert("RGBA")
    if size:
        im = im.resize(size, Image.LANCZOS)
    B.paste(im, xy, im)
put(f"{DIST}/logo/odm-wordmark-on-dark@2x.png", (30, 30), None)
put(f"{DIST}/logo/odm-wordmark-on-light@2x.png", (730, 30), None)
x = 30
for s in (256, 128, 64, 48, 32, 24, 16):
    put(f"{DIST}/app-icons/app-icon.png" if s > 64 else None, (x, 200), (s, s)) if False else None
from PIL import Image as I
import subprocess
x = 30
for s, f in ((256, "app-icons/icon.png"), (128, "app-icons/128x128.png"), (64, "app-icons/linux/hicolor/64x64/apps/open-download-manager.png"),
             (48, "app-icons/linux/hicolor/48x48/apps/open-download-manager.png"), (32, "app-icons/32x32.png"),
             (24, "app-icons/linux/hicolor/24x24/apps/open-download-manager.png"), (16, "app-icons/linux/hicolor/16x16/apps/open-download-manager.png")):
        put(f"{DIST}/{f}", (x, 200), (s, s))
        x += s + 20
put(f"{DIST}/app-icons/macos-1024.png", (730, 170), (300, 300))
# tray
y = 500
x = 30
for st in ("idle", "downloading", "paused", "error", "complete", "limited"):
    put(f"{DIST}/tray/windows-linux/tray-{st}-32.png", (x, y), (64, 64))
    put(f"{DIST}/tray/windows-linux/tray-{st}-16.png", (x + 72, y + 24))
    put(f"{DIST}/tray/windows-linux/tray-{st}-32.png", (x + 72, y + 44))
    x += 110
x = 730
for st in ("idle", "downloading", "paused", "error", "complete", "limited"):
    put(f"{DIST}/tray/macos/tray-{st}Template@2x.png", (x, y), (72, 72))
    put(f"{DIST}/tray/macos/tray-{st}Template.png", (x + 78, y + 30))
    x += 108
put(f"{DIST}/browser-extension/download-panel-button@2x.png", (30, 620))
put(f"{DIST}/installer/dmg-background.png", (730, 620), (330, 200))
put(f"{DIST}/installer/nsis-sidebar.bmp", (30, 700), (82, 157))
put(f"{DIST}/installer/wix-dialog.bmp", (130, 700), (246, 156))
put(f"{DIST}/installer/wix-banner.bmp", (390, 700), (246, 29))
put(f"{DIST}/installer/nsis-header.bmp", (390, 740))
B.save(f"{OUT}/brand-sheet.png")
print("ok")
