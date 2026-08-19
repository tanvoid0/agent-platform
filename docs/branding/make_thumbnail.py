from PIL import Image, ImageDraw, ImageFont

W, H = 1280, 800
bg_top = (13, 17, 23)
bg_bottom = (25, 20, 45)
accent = (124, 92, 255)
accent2 = (60, 200, 160)
white = (240, 240, 245)
gray = (150, 155, 165)

img = Image.new("RGB", (W, H), bg_top)
d = ImageDraw.Draw(img)

for y in range(H):
    t = y / H
    r = int(bg_top[0] + (bg_bottom[0] - bg_top[0]) * t)
    g = int(bg_top[1] + (bg_bottom[1] - bg_top[1]) * t)
    b = int(bg_top[2] + (bg_bottom[2] - bg_top[2]) * t)
    d.line([(0, y), (W, y)], fill=(r, g, b))

def font(path, size):
    return ImageFont.truetype(path, size)

f_title = font("/c/Windows/Fonts/segoeuib.ttf".replace("/c/", "C:/"), 64)
f_sub = font("/c/Windows/Fonts/segoeui.ttf".replace("/c/", "C:/"), 30)
f_tag = font("/c/Windows/Fonts/segoeuib.ttf".replace("/c/", "C:/"), 22)
f_small = font("/c/Windows/Fonts/segoeui.ttf".replace("/c/", "C:/"), 20)

# node graph motif (agents talking to each other)
import math
cx, cy = 980, 250
nodes = []
for i, ang in enumerate([0, 60, 130, 200, 270, 330]):
    rad = 110 if i % 2 == 0 else 70
    x = cx + rad * math.cos(math.radians(ang))
    y = cy + rad * math.sin(math.radians(ang)) * 0.7
    nodes.append((x, y))

for i in range(len(nodes)):
    for j in range(i + 1, len(nodes)):
        d.line([nodes[i], nodes[j]], fill=(70, 60, 110), width=2)

d.ellipse([cx - 14, cy - 14, cx + 14, cy + 14], fill=accent)
for i, (x, y) in enumerate(nodes):
    col = accent2 if i % 2 == 0 else accent
    r = 10
    d.ellipse([x - r, y - r, x + r, y + r], fill=col)

# left margin accent bar
d.rectangle([0, 0, 10, H], fill=accent)

# text block
d.text((90, 120), "AGENT PLATFORM", font=f_title, fill=white)
d.text((90, 200), "Multi-agent orchestration", font=f_sub, fill=gray)

tags = ["Rust", "iced", "Native", "BYOK", "Multi-agent"]
tx = 90
ty = 280
for tag in tags:
    tw = d.textlength(tag, font=f_tag)
    pad = 18
    d.rounded_rectangle([tx, ty, tx + tw + pad * 2, ty + 44], radius=22, outline=accent, width=2)
    d.text((tx + pad, ty + 10), tag, font=f_tag, fill=white)
    tx += tw + pad * 2 + 14

d.text((90, H - 90), "github.com/tanvoid0/agent-platform", font=f_small, fill=gray)

img.save(r"D:\production\ai\agentic-ai\agent-platform\docs\branding\thumbnail.png")
print("saved")
