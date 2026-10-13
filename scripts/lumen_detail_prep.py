"""Prepare the detail textures for lumen_detail.py from the source sprite.

Outputs (into the sprite-mcp models/ directory):
  seraphine_face.png   Her face, cropped from her own sprite (bangs,
                       glasses, eyes, smile), transparent margins filled
                       with her cheek tone, upscaled for projection onto
                       the model's head front.
  seraphine_chest.png  A torso print drawn from her sampled palette:
                       grey hoodie, khaki vest panels and straps, waist
                       buckle, blue/white chest badge.

Palette points are sampled from the sprite itself and printed, so the
textures carry her colors rather than an artist's guess.

Run: python3 scripts/lumen_detail_prep.py <sprite.png> <models_dir>
"""

import sys

from PIL import Image, ImageDraw

# Her face in the source sprite (frame 0), cross-checked against the
# identity lock: hairline y=97, chin y=154, face span 58 px.
FACE_RECT = (156, 82, 214, 160)
CHEEK_POINT = (192, 138)     # Skin tone for seam filling.
HAIR_POINT = (120, 60)       # Lit magenta.
# Vest and hoodie are warm greys on the sprite (verified by grid sample
# 2026-10-06; the vest reads khaki only by contrast with the cool greys
# around it). Values sampled from the vest panel / hoodie body.
VEST_RGB = (150, 140, 136)
HOODIE_RGB = (210, 203, 198)


def sample(image, point):
    return image.getpixel(point)[:3]


def build_face(sprite):
    crop = sprite.crop(FACE_RECT).convert("RGBA")
    skin = sample(sprite, CHEEK_POINT)
    filled = Image.new("RGBA", crop.size, skin + (255,))
    filled.paste(crop, (0, 0), crop)
    return filled.resize((464, 624), Image.LANCZOS)


def build_chest(sprite):
    hoodie = HOODIE_RGB
    vest = VEST_RGB
    dark_vest = tuple(int(c * 0.72) for c in vest)
    img = Image.new("RGBA", (512, 640), hoodie + (255,))
    d = ImageDraw.Draw(img)
    # Hoodie shading: darker toward the hem.
    for y in range(640):
        shade = 1.0 - 0.13 * (y / 640)
        d.line([(0, y), (512, y)],
               fill=tuple(int(c * shade) for c in hoodie) + (255,))
    # Vest side panels and shoulder straps (khaki), open at the center.
    d.rectangle([26, 30, 150, 500], fill=vest + (255,))
    d.rectangle([362, 30, 486, 500], fill=vest + (255,))
    d.rectangle([26, 30, 150, 500], outline=dark_vest + (255,), width=5)
    d.rectangle([362, 30, 486, 500], outline=dark_vest + (255,), width=5)
    # Horizontal waist strap with a center buckle.
    d.rectangle([26, 452, 486, 512], fill=dark_vest + (255,))
    d.rectangle([216, 458, 296, 506], outline=(90, 88, 92, 255), width=7)
    # Center zipper line on the hoodie.
    d.line([(256, 20), (256, 452)], fill=(105, 100, 102, 255), width=4)
    # Drawstrings.
    d.line([(226, 8), (218, 74)], fill=(238, 236, 234, 255), width=5)
    d.line([(286, 8), (294, 74)], fill=(238, 236, 234, 255), width=5)
    # Blue/white chest badge on her left chest.
    d.rounded_rectangle([330, 120, 392, 168], radius=7,
                        fill=(232, 236, 240, 255), outline=(70, 90, 140, 255), width=3)
    d.rectangle([352, 128, 370, 160], fill=(52, 92, 168, 255))
    d.rectangle([338, 137, 384, 151], fill=(52, 92, 168, 255))
    return img


def main():
    sprite_path, models_dir = sys.argv[1], sys.argv[2]
    sprite = Image.open(sprite_path).convert("RGBA")
    for name, point in (("cheek", CHEEK_POINT), ("hair", HAIR_POINT)):
        print(f"PALETTE {name} {sample(sprite, point)}")
    print(f"PALETTE vest {VEST_RGB}")
    print(f"PALETTE hoodie {HOODIE_RGB}")
    face_path = f"{models_dir}/seraphine_face.png"
    build_face(sprite).save(face_path)
    print(f"PREP {face_path}")
    chest_path = f"{models_dir}/seraphine_chest.png"
    build_chest(sprite).save(chest_path)
    print(f"PREP {chest_path}")


if __name__ == "__main__":
    main()
