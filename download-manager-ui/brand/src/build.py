#!/usr/bin/env python3
"""Build every Open Download Manager graphic from src/icons.json.

Usage: python3 src/build.py <out_dir>
Requires: cairosvg, Pillow, fontTools, and IBM Plex Sans SemiBold OTF at $PLEX_SEMIBOLD.
"""
import io, json, os, sys, math
import cairosvg
from PIL import Image, ImageFilter, ImageChops, ImageDraw
from fontTools.ttLib import TTFont
from fontTools.pens.svgPathPen import SVGPathPen
from fontTools.pens.transformPen import TransformPen

HERE = os.path.dirname(os.path.abspath(__file__))
OUT = os.path.abspath(sys.argv[1] if len(sys.argv) > 1 else "dist")
# IBM Plex Sans SemiBold (OFL), e.g. from https://github.com/IBM/plex
PLEX = os.environ.get("PLEX_SEMIBOLD", os.path.join(HERE, "fonts", "IBMPlexSans-SemiBold.otf"))

# Brand colors (match design/tokens.css)
ACCENT = "#5B9DFF"
INK = "#0A1426"
DARK_BG = "#0D0F12"
DARK_SURFACE = "#14171C"
TEXT_DARK = "#E7E9EC"
TEXT_LIGHT = "#14171C"
STATE = {"downloading": "#5B9DFF", "paused": "#F0A04B", "error": "#F26B75", "complete": "#4CC38A", "limited": "#F0A04B"}

DATA = json.load(open(os.path.join(HERE, "icons.json")))
ICONS = {}
for g in DATA["groups"]:
    for icon_id, label, idm, shapes in g["icons"]:
        ICONS[icon_id] = dict(group=g["id"], label=label, idm=idm, shapes=shapes)


def w(path, data, mode="w"):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, mode) as f:
        f.write(data)


def circle_d(cx, cy, r):
    return f"M{cx - r:g} {cy:g}a{r:g} {r:g} 0 1 0 {2 * r:g} 0a{r:g} {r:g} 0 1 0 {-2 * r:g} 0z"


def shape_elems(shapes, color="currentColor", sw=2.0):
    out = []
    for s in shapes:
        if s.startswith("O "):
            cx, cy, r = map(float, s[2:].split())
            out.append(f'<path d="{circle_d(cx, cy, r)}" fill="none" stroke="{color}" stroke-width="{sw:g}"/>')
        elif s.startswith("D "):
            cx, cy, r = map(float, s[2:].split())
            out.append(f'<path d="{circle_d(cx, cy, r)}" fill="{color}" stroke="none"/>')
        elif s.startswith("F "):
            out.append(f'<path d="{s[2:]}" fill="{color}" stroke="none"/>')
        else:
            out.append(f'<path d="{s}" fill="none" stroke="{color}" stroke-width="{sw:g}"/>')
    return "".join(out)


def icon_svg(icon_id, color="currentColor", sw=2.0, size=24):
    body = shape_elems(ICONS[icon_id]["shapes"], color, sw)
    return (f'<svg xmlns="http://www.w3.org/2000/svg" width="{size}" height="{size}" viewBox="0 0 24 24" '
            f'stroke-linecap="round" stroke-linejoin="round">{body}</svg>')


def png(svg, size, h=None):
    data = cairosvg.svg2png(bytestring=svg.encode(), output_width=size, output_height=h or size)
    return Image.open(io.BytesIO(data)).convert("RGBA")


MARK = ICONS["odm-mark"]["shapes"]


def mark_group(x, y, scale, color, sw):
    return (f'<g transform="translate({x:g} {y:g}) scale({scale:g})" stroke-linecap="round" stroke-linejoin="round">'
            f'{shape_elems(MARK, color, sw)}</g>')


def tile_svg(size, padded=False):
    """App icon. padded=True -> macOS grid (824 tile inside 1024 canvas)."""
    if padded:
        inset = size * 100 / 1024
        t = size - 2 * inset
        radius = t * 0.2245
    else:
        inset, t = 0, size
        radius = t * 0.22
    small = size <= 32
    mark_frac = 0.74 if small else 0.64
    sw = 2.7 if small else 2.25
    scale = t * mark_frac / 24
    off = inset + (t - 24 * scale) / 2
    return (f'<svg xmlns="http://www.w3.org/2000/svg" width="{size}" height="{size}" viewBox="0 0 {size} {size}">'
            f'<rect x="{inset:g}" y="{inset:g}" width="{t:g}" height="{t:g}" rx="{radius:g}" fill="{ACCENT}"/>'
            f'{mark_group(off, off, scale, INK, sw)}</svg>')


def mac_icon(size):
    """Padded tile with the soft drop shadow macOS icons carry."""
    base = png(tile_svg(size, padded=True), size)
    alpha = base.split()[3]
    shadow = Image.new("RGBA", base.size, (0, 0, 0, 0))
    sh_alpha = alpha.point(lambda a: int(a * 0.32)).filter(ImageFilter.GaussianBlur(size * 0.012))
    shadow.putalpha(sh_alpha)
    canvas = Image.new("RGBA", base.size, (0, 0, 0, 0))
    canvas.alpha_composite(shadow, (0, max(1, int(size * 0.01))))
    canvas.alpha_composite(base)
    return canvas


# ---------- wordmark (outlined, no font dependency) ----------
def text_path(text, font_path, size_px):
    font = TTFont(font_path)
    gs = font.getGlyphSet()
    cmap = font.getBestCmap()
    upm = font["head"].unitsPerEm
    hmtx = font["hmtx"]
    s = size_px / upm
    pen = SVGPathPen(gs)
    x = 0
    for ch in text:
        gname = cmap[ord(ch)]
        tp = TransformPen(pen, (s, 0, 0, -s, x, 0))
        gs[gname].draw(tp)
        x += hmtx[gname][0] * s
    asc = font["OS/2"].sCapHeight * s
    return pen.getCommands(), x, asc


def wordmark_svg(text_color, mark_tile=True, layout="horizontal"):
    name = "Open Download Manager"
    fs = 40
    d, tw, cap = text_path(name, PLEX, fs)
    if layout == "horizontal":
        icon = 64
        gap = 18
        W = icon + gap + tw + 6
        H = icon
        ty = H / 2 + cap / 2
        tile = tile_svg(icon).split(">", 1)[1].rsplit("</svg>", 1)[0]
        return (f'<svg xmlns="http://www.w3.org/2000/svg" width="{W:.0f}" height="{H}" viewBox="0 0 {W:.1f} {H}">'
                f'<g>{tile}</g><path transform="translate({icon + gap} {ty:.2f})" d="{d}" fill="{text_color}"/></svg>')
    # stacked
    icon = 128
    W = max(tw, icon) + 8
    H = icon + 28 + cap + 8
    tile = tile_svg(icon).split(">", 1)[1].rsplit("</svg>", 1)[0]
    return (f'<svg xmlns="http://www.w3.org/2000/svg" width="{W:.0f}" height="{H:.0f}" viewBox="0 0 {W:.1f} {H:.1f}">'
            f'<g transform="translate({(W - icon) / 2:.1f} 0)">{tile}</g>'
            f'<path transform="translate({(W - tw) / 2:.1f} {icon + 28 + cap:.1f})" d="{d}" fill="{text_color}"/></svg>')


def mark_only_svg(color, size=24, sw=2.2):
    return (f'<svg xmlns="http://www.w3.org/2000/svg" width="{size}" height="{size}" viewBox="0 0 24 24" '
            f'stroke-linecap="round" stroke-linejoin="round">{shape_elems(MARK, color, sw)}</svg>')


# ---------- tray ----------
BADGE_GLYPH = {
    "downloading": ['M18.5 15.6v5.2', 'M16.4 18.8l2.1 2.1 2.1-2.1'],
    "paused": ['M17.3 16.3v4.4', 'M19.7 16.3v4.4'],
    "error": ['M18.5 15.6v3.1', 'D 18.5 21 0.9'],
    "complete": ['M16.2 18.6l1.6 1.6 3-3.3'],
    "limited": ['M16.2 18.5h4.6'],
}


def tray_images(state, px, template):
    """Returns RGBA image. template=True -> black+alpha (macOS); else colored tile + colored badge."""
    if template:
        base = png(mark_only_svg("#000000", sw=2.6), px)
    else:
        base = png(tile_svg(px), px)
    if state == "idle":
        return base
    k = px / 24
    cx, cy, rb, rk = 18.5 * k, 18.5 * k, 5.2 * k, 6.6 * k
    # knock out space around badge
    knock = Image.new("L", (px, px), 255)
    ImageDraw.Draw(knock).ellipse([cx - rk, cy - rk, cx + rk, cy + rk], fill=0)
    r, g, b, a = base.split()
    base.putalpha(ImageChops.multiply(a, knock))
    glyph_svg = (f'<svg xmlns="http://www.w3.org/2000/svg" width="{px}" height="{px}" viewBox="0 0 24 24" '
                 f'stroke-linecap="round" stroke-linejoin="round">{shape_elems(BADGE_GLYPH[state], "#000", 1.7)}</svg>')
    glyph_a = png(glyph_svg, px).split()[3]
    disc = Image.new("L", (px, px), 0)
    ImageDraw.Draw(disc).ellipse([cx - rb, cy - rb, cx + rb, cy + rb], fill=255)
    if template:
        badge_a = ImageChops.subtract(disc, glyph_a)
        badge = Image.new("RGBA", (px, px), (0, 0, 0, 0))
        badge.putalpha(badge_a)
    else:
        col = Image.new("RGBA", (px, px), STATE[state])
        badge = Image.new("RGBA", (px, px), (0, 0, 0, 0))
        badge.paste(col, (0, 0), disc)
        white = Image.new("RGBA", (px, px), "#FFFFFF" if state != "complete" and state != "limited" and state != "paused" else INK)
        badge.paste(white, (0, 0), ImageChops.multiply(glyph_a, disc))
    out = base.copy()
    out.alpha_composite(badge)
    return out


# ---------- download panel button (IDM's "Download this video" overlay) ----------
def panel_svg():
    d, tw, cap = text_path("Download with ODM", PLEX, 13)
    W = 12 + 18 + 8 + tw + 14 + 1 + 10 + 12 + 10
    H = 34
    mark = mark_group(12, 8, 18 / 24, INK, 2.4)
    tx = 12 + 18 + 8
    sep = tx + tw + 14
    chev = f'<path d="M{sep + 10:.1f} 15l4 4 4-4" fill="none" stroke="{INK}" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"/>'
    return (f'<svg xmlns="http://www.w3.org/2000/svg" width="{W:.0f}" height="{H}" viewBox="0 0 {W:.1f} {H}">'
            f'<rect width="{W:.1f}" height="{H}" rx="9" fill="{ACCENT}"/>{mark}'
            f'<path transform="translate({tx} {H / 2 + cap / 2:.2f})" d="{d}" fill="{INK}"/>'
            f'<path d="M{sep:.1f} 9v16" stroke="{INK}" stroke-opacity="0.35" stroke-width="1"/>{chev}</svg>')


# ---------- installer art ----------
def installer_svg(W, H, kind):
    name_d, tw, cap = text_path("Open Download Manager", PLEX, 18 if kind in ("banner", "header") else 22)
    if kind in ("banner", "header"):
        icon = H - 18
        tile = tile_svg(int(icon)).split(">", 1)[1].rsplit("</svg>", 1)[0]
        x_icon = W - icon - 10
        txt = f'<path transform="translate(14 {H / 2 + cap / 2:.1f})" d="{name_d}" fill="{TEXT_LIGHT}"/>' if kind == "banner" else ""
        return (f'<svg xmlns="http://www.w3.org/2000/svg" width="{W}" height="{H}" viewBox="0 0 {W} {H}">'
                f'<rect width="{W}" height="{H}" fill="#FFFFFF"/>{txt}'
                f'<g transform="translate({x_icon} 9)">{tile}</g></svg>')
    # sidebar / dialog / dmg: dark brand panel
    if kind == "sidebar":
        icon = 96
        tile = tile_svg(icon).split(">", 1)[1].rsplit("</svg>", 1)[0]
        return (f'<svg xmlns="http://www.w3.org/2000/svg" width="{W}" height="{H}" viewBox="0 0 {W} {H}">'
                f'<rect width="{W}" height="{H}" fill="{DARK_BG}"/>'
                f'<g transform="translate({(W - icon) / 2} 64)">{tile}</g>'
                + ring_motif(W / 2, H - 40, 120) + '</svg>')
    if kind == "dialog":  # WiX dialog: left 164px is art, rest white for text
        icon = 88
        tile = tile_svg(icon).split(">", 1)[1].rsplit("</svg>", 1)[0]
        return (f'<svg xmlns="http://www.w3.org/2000/svg" width="{W}" height="{H}" viewBox="0 0 {W} {H}">'
                f'<rect width="{W}" height="{H}" fill="#FFFFFF"/><rect width="164" height="{H}" fill="{DARK_BG}"/>'
                f'<g transform="translate({(164 - icon) / 2} 48)">{tile}</g>' + ring_motif(82, H - 30, 110) + '</svg>')
    if kind == "dmg":
        return (f'<svg xmlns="http://www.w3.org/2000/svg" width="{W}" height="{H}" viewBox="0 0 {W} {H}">'
                f'<rect width="{W}" height="{H}" fill="{DARK_BG}"/>'
                f'<path transform="translate({(W - tw) / 2:.1f} 70)" d="{name_d}" fill="{TEXT_DARK}"/>'
                f'<path d="M290 215h80" stroke="#98A1AD" stroke-width="2.5" stroke-linecap="round"/>'
                f'<path d="M360 205l10 10-10 10" fill="none" stroke="#98A1AD" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"/>'
                f'<text x="{W / 2}" y="355" text-anchor="middle" font-family="IBM Plex Sans, Helvetica, sans-serif" font-size="13" fill="#98A1AD">Drag to Applications to install</text></svg>')
    raise ValueError(kind)


def ring_motif(cx, cy, r):
    """Large faint open-ring echo of the mark for installer side panels."""
    k = r / 8
    gx, gy = cx - 12 * k, cy - 13.5 * k
    return (f'<g transform="translate({gx:g} {gy:g}) scale({k:g})" opacity="0.10" stroke-linecap="round" stroke-linejoin="round">'
            f'{shape_elems(MARK, ACCENT, 1.2)}</g>')


def _png_bytes(img):
    b = io.BytesIO()
    img.save(b, "PNG")
    return b.getvalue()


def write_ico(path, images):
    """ICO with PNG-compressed entries, written in the given order."""
    import struct
    blobs = [_png_bytes(im) for im in images]
    head = struct.pack("<HHH", 0, 1, len(images))
    offset = 6 + 16 * len(images)
    entries = b""
    for im, blob in zip(images, blobs):
        wdt, hgt = im.size
        entries += struct.pack("<BBBBHHII", wdt % 256, hgt % 256, 0, 0, 1, 32, len(blob), offset)
        offset += len(blob)
    with open(path, "wb") as f:
        f.write(head + entries + b"".join(blobs))


def write_icns(path, images_by_type):
    """ICNS with PNG payloads. images_by_type: list of (OSType, Image)."""
    import struct
    chunks = b""
    for ostype, im in images_by_type:
        blob = _png_bytes(im)
        chunks += ostype.encode() + struct.pack(">I", len(blob) + 8) + blob
    with open(path, "wb") as f:
        f.write(b"icns" + struct.pack(">I", len(chunks) + 8) + chunks)


def save_bmp(img, path, bg="#FFFFFF"):
    flat = Image.new("RGB", img.size, bg)
    flat.paste(img, (0, 0), img)
    os.makedirs(os.path.dirname(path), exist_ok=True)
    flat.save(path, "BMP")


def main():
    os.makedirs(OUT, exist_ok=True)

    # 1. Icon library SVGs + sprite
    symbols = []
    manifest = []
    for icon_id, meta in ICONS.items():
        if icon_id == "odm-mark":
            continue
        w(f"{OUT}/icons/svg/{icon_id}.svg", icon_svg(icon_id))
        body = shape_elems(meta["shapes"])
        symbols.append(f'<symbol id="{icon_id}" viewBox="0 0 24 24" stroke-linecap="round" stroke-linejoin="round">{body}</symbol>')
        manifest.append({"id": icon_id, "label": meta["label"], "group": meta["group"], "idm": meta["idm"]})
        for px in (16, 24, 32, 48):
            for tone, col in (("light", "#E7E9EC"), ("dark", "#14171C")):
                d = f"{OUT}/icons/png/{tone}/{px}"
                os.makedirs(d, exist_ok=True)
                png(icon_svg(icon_id, col), px).save(f"{d}/{icon_id}.png")
    w(f"{OUT}/icons/sprite.svg",
      '<svg xmlns="http://www.w3.org/2000/svg" style="display:none">' + "".join(symbols) + "</svg>")
    w(f"{OUT}/icons/manifest.json", json.dumps(manifest, indent=2))

    # 2. Logo files
    w(f"{OUT}/logo/odm-mark.svg", mark_only_svg("currentColor"))
    w(f"{OUT}/logo/odm-mark-accent.svg", mark_only_svg(ACCENT))
    w(f"{OUT}/logo/odm-app-icon.svg", tile_svg(1024))
    w(f"{OUT}/logo/odm-app-icon-macos.svg", tile_svg(1024, padded=True))
    for tone, col in (("on-dark", TEXT_DARK), ("on-light", TEXT_LIGHT)):
        hz = wordmark_svg(col)
        st = wordmark_svg(col, layout="stacked")
        w(f"{OUT}/logo/odm-wordmark-{tone}.svg", hz)
        w(f"{OUT}/logo/odm-lockup-stacked-{tone}.svg", st)
    # PNG of logos for docs/readme
    for tone in ("on-dark", "on-light"):
        svg = open(f"{OUT}/logo/odm-wordmark-{tone}.svg").read()
        Wd = float(svg.split('viewBox="0 0 ')[1].split()[0])
        png(svg, int(Wd * 2), 128).save(f"{OUT}/logo/odm-wordmark-{tone}@2x.png")

    # 3. App icons (Tauri src-tauri/icons layout)
    ai = f"{OUT}/app-icons"
    os.makedirs(ai, exist_ok=True)
    src = png(tile_svg(1024), 1024)
    src.save(f"{ai}/app-icon.png")          # input for `tauri icon`
    png(tile_svg(32), 32).save(f"{ai}/32x32.png")
    png(tile_svg(128), 128).save(f"{ai}/128x128.png")
    png(tile_svg(256), 256).save(f"{ai}/128x128@2x.png")
    png(tile_svg(512), 512).save(f"{ai}/icon.png")
    for n in (30, 44, 71, 89, 107, 142, 150, 284, 310):
        png(tile_svg(n), n).save(f"{ai}/Square{n}x{n}Logo.png")
    png(tile_svg(50), 50).save(f"{ai}/StoreLogo.png")
    ico_sizes = [32, 16, 24, 48, 64, 256]
    write_ico(f"{ai}/icon.ico", [png(tile_svg(s), s) for s in ico_sizes])
    mi = {s: (mac_icon(s) if s >= 128 else png(tile_svg(s, padded=True), s)) for s in (16, 32, 64, 128, 256, 512, 1024)}
    write_icns(f"{ai}/icon.icns", [("ic04", mi[16]), ("ic05", mi[32]), ("ic11", mi[32]), ("ic12", mi[64]),
                                   ("ic07", mi[128]), ("ic13", mi[256]), ("ic08", mi[256]), ("ic14", mi[512]),
                                   ("ic09", mi[512]), ("ic10", mi[1024])])
    mac_icon(1024).save(f"{ai}/macos-1024.png")
    # Linux hicolor set
    for s in (16, 24, 32, 48, 64, 128, 256, 512):
        os.makedirs(f"{ai}/linux/hicolor/{s}x{s}/apps", exist_ok=True)
        png(tile_svg(s), s).save(f"{ai}/linux/hicolor/{s}x{s}/apps/open-download-manager.png")

    # 4. Tray icons
    os.makedirs(f"{OUT}/tray/windows-linux", exist_ok=True)
    for state in ("idle", "downloading", "paused", "error", "complete", "limited"):
        tray_images(state, 16, False).save(f"{OUT}/tray/windows-linux/tray-{state}-16.png")
        tray_images(state, 32, False).save(f"{OUT}/tray/windows-linux/tray-{state}-32.png")
        os.makedirs(f"{OUT}/tray/macos", exist_ok=True)
        tray_images(state, 18, True).save(f"{OUT}/tray/macos/tray-{state}Template.png")
        tray_images(state, 36, True).save(f"{OUT}/tray/macos/tray-{state}Template@2x.png")
    # Windows .ico per state (16/24/32) for SetIcon-style APIs
    for state in ("idle", "downloading", "paused", "error", "complete", "limited"):
        write_ico(f"{OUT}/tray/windows-linux/tray-{state}.ico", [tray_images(state, s, False) for s in (16, 24, 32)])

    # 5. Browser extension icons (Chrome/Edge/Firefox manifest sizes)
    for s in (16, 19, 32, 38, 48, 96, 128):
        os.makedirs(f"{OUT}/browser-extension", exist_ok=True)
        png(tile_svg(s), s).save(f"{OUT}/browser-extension/icon-{s}.png")
    for s in (16, 32):  # disabled / capture-paused state: grey tile
        svg = tile_svg(s).replace(ACCENT, "#5C6470")
        png(svg, s).save(f"{OUT}/browser-extension/icon-{s}-paused.png")
    w(f"{OUT}/browser-extension/download-panel-button.svg", panel_svg())
    p = panel_svg()
    pw = float(p.split('viewBox="0 0 ')[1].split()[0])
    png(p, int(pw * 2), 68).save(f"{OUT}/browser-extension/download-panel-button@2x.png")

    # 6. Installer art
    inst = f"{OUT}/installer"
    save_bmp(png(installer_svg(150, 57, "header"), 150, 57), f"{inst}/nsis-header.bmp")
    save_bmp(png(installer_svg(164, 314, "sidebar"), 164, 314), f"{inst}/nsis-sidebar.bmp", DARK_BG)
    save_bmp(png(installer_svg(493, 58, "banner"), 493, 58), f"{inst}/wix-banner.bmp")
    save_bmp(png(installer_svg(493, 312, "dialog"), 493, 312), f"{inst}/wix-dialog.bmp")
    dmg = installer_svg(660, 400, "dmg")
    png(dmg, 660, 400).save(f"{inst}/dmg-background.png")
    png(dmg, 1320, 800).save(f"{inst}/dmg-background@2x.png")

    # 7. Code: typed icon map for the Svelte frontend
    def to_els(shapes):
        out = []
        for s in shapes:
            if s[:2] in ("O ", "D "):
                cx, cy, r = map(float, s[2:].split())
                out.append({"d": circle_d(cx, cy, r), "fill": s[0] == "D"})
            elif s.startswith("F "):
                out.append({"d": s[2:], "fill": True})
            else:
                out.append({"d": s, "fill": False})
        return out
    entries = {k: to_els(v["shapes"]) for k, v in ICONS.items()}
    ts = ["// Generated by src/build.py from src/icons.json. Do not edit by hand.",
          "export type IconShape = { d: string; fill: boolean };",
          "export const ICONS = " + json.dumps(entries, indent=1) + " as const satisfies Record<string, readonly IconShape[]>;",
          "export type IconName = keyof typeof ICONS;", ""]
    w(f"{OUT}/code/icons.ts", "\n".join(ts))
    w(f"{OUT}/code/Icon.svelte", """<script lang="ts">
  import { ICONS, type IconName } from './icons';
  let { name, size = 16, stroke = 2, label = undefined }:
    { name: IconName; size?: number; stroke?: number; label?: string } = $props();
</script>

<svg width={size} height={size} viewBox="0 0 24 24" fill="none" stroke="currentColor"
     stroke-width={stroke} stroke-linecap="round" stroke-linejoin="round"
     role={label ? 'img' : undefined} aria-label={label} aria-hidden={label ? undefined : 'true'}>
  {#each ICONS[name] as s}
    {#if s.fill}<path d={s.d} fill="currentColor" stroke="none" />{:else}<path d={s.d} />{/if}
  {/each}
</svg>
""")
    w(f"{OUT}/code/tray.rs", """// Tray icon state switching for Tauri 2.
// Copy tray/windows-linux/*.png and tray/macos/*Template*.png into src-tauri/icons/tray/.
use tauri::{image::Image, tray::TrayIcon, Runtime};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrayState { Idle, Downloading, Paused, Error, Complete, Limited }

impl TrayState {
    fn key(self) -> &'static str {
        match self {
            TrayState::Idle => "idle",
            TrayState::Downloading => "downloading",
            TrayState::Paused => "paused",
            TrayState::Error => "error",
            TrayState::Complete => "complete",
            TrayState::Limited => "limited",
        }
    }
}

/// Priority when several apply: Error > Downloading > Limited (while active) > Paused > Complete (5 s) > Idle.
pub fn set_tray_state<R: Runtime>(tray: &TrayIcon<R>, state: TrayState) -> tauri::Result<()> {
    #[cfg(target_os = "macos")]
    let bytes: &[u8] = match state {
        TrayState::Idle => include_bytes!("../icons/tray/tray-idleTemplate@2x.png"),
        TrayState::Downloading => include_bytes!("../icons/tray/tray-downloadingTemplate@2x.png"),
        TrayState::Paused => include_bytes!("../icons/tray/tray-pausedTemplate@2x.png"),
        TrayState::Error => include_bytes!("../icons/tray/tray-errorTemplate@2x.png"),
        TrayState::Complete => include_bytes!("../icons/tray/tray-completeTemplate@2x.png"),
        TrayState::Limited => include_bytes!("../icons/tray/tray-limitedTemplate@2x.png"),
    };
    #[cfg(not(target_os = "macos"))]
    let bytes: &[u8] = match state {
        TrayState::Idle => include_bytes!("../icons/tray/tray-idle-32.png"),
        TrayState::Downloading => include_bytes!("../icons/tray/tray-downloading-32.png"),
        TrayState::Paused => include_bytes!("../icons/tray/tray-paused-32.png"),
        TrayState::Error => include_bytes!("../icons/tray/tray-error-32.png"),
        TrayState::Complete => include_bytes!("../icons/tray/tray-complete-32.png"),
        TrayState::Limited => include_bytes!("../icons/tray/tray-limited-32.png"),
    };
    tray.set_icon(Some(Image::from_bytes(bytes)?))?;
    #[cfg(target_os = "macos")]
    tray.set_icon_as_template(true)?;
    let _ = state.key();
    Ok(())
}
""")

    print("built", len(manifest), "icons into", OUT)


if __name__ == "__main__":
    main()
