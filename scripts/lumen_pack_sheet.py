"""Assemble lumen_render_sheet.py output into deliverable 2D artifacts.

Inputs (from the render directory): idle_frame_00..11.png (288x576 RGBA)
and expr_<name>.png stills (576x1152). Outputs, beside the inputs:
  seraphine_mature_roundtrip.apng  12-frame idle, 200 ms/frame, looping —
                                   the same format as the source sprite.
  seraphine_roundtrip_strip.png    the 12 frames in one row, for review.
  seraphine_mature_idle.gif        144x288 preview, matching the base set.
  seraphine_expressions.png        labeled head-crop contact sheet:
                                   neutral + 10 emotions + wink + celebrate.

Run: python3 scripts/lumen_pack_sheet.py models/renders2d
"""

import os
import sys

from PIL import Image, ImageDraw

FRAME_MS = 200
EXPRESSIONS = (
    "neutral", "happy", "angry", "sad", "surprised", "fun", "annoyed",
    "worried", "embarrassed", "determined", "smug", "wink", "celebrate",
)
CELL_W, CROP_H, LABEL_H = 288, 252, 34


def alpha_bbox(image):
    return image.getchannel("A").getbbox()


def head_crop(still):
    """Top-of-subject crop: hair + head, centered on the subject."""
    box = alpha_bbox(still)
    assert box, "empty render"
    left, top, right, bottom = box
    subject_h = bottom - top
    crop_h = int(subject_h * 0.24)
    center_x = (left + right) // 2
    half = crop_h // 2
    x0 = max(0, center_x - half)
    x1 = min(still.width, x0 + crop_h)
    cell = still.crop((x0, top, x1, top + crop_h))
    return cell.resize((CELL_W, CROP_H), Image.LANCZOS)


def main():
    out_dir = sys.argv[1]
    frames = [Image.open(os.path.join(out_dir, f"idle_frame_{i:02d}.png")).convert("RGBA")
              for i in range(12)]
    assert all(f.size == (288, 576) for f in frames), "idle frames must be 288x576"

    apng = os.path.join(out_dir, "seraphine_mature_roundtrip.apng")
    frames[0].save(apng, save_all=True, append_images=frames[1:],
                   duration=FRAME_MS, loop=0)
    print(f"PACK {apng}")

    strip = Image.new("RGBA", (288 * 12, 576), (0, 0, 0, 0))
    for i, frame in enumerate(frames):
        strip.paste(frame, (288 * i, 0))
    strip_path = os.path.join(out_dir, "seraphine_roundtrip_strip.png")
    strip.save(strip_path)
    print(f"PACK {strip_path}")

    gif_frames = [f.resize((144, 288), Image.LANCZOS) for f in frames]
    gif_path = os.path.join(out_dir, "seraphine_mature_idle.gif")
    gif_frames[0].save(gif_path, save_all=True, append_images=gif_frames[1:],
                       duration=FRAME_MS, loop=0, transparency=0,
                       disposal=2)
    print(f"PACK {gif_path}")

    cols = 5
    rows = (len(EXPRESSIONS) + cols - 1) // cols
    sheet = Image.new("RGBA", (cols * CELL_W, rows * (CROP_H + LABEL_H)),
                      (24, 24, 28, 255))
    draw = ImageDraw.Draw(sheet)
    for idx, name in enumerate(EXPRESSIONS):
        still = Image.open(os.path.join(out_dir, f"expr_{name}.png")).convert("RGBA")
        cell = head_crop(still)
        x = (idx % cols) * CELL_W
        y = (idx // cols) * (CROP_H + LABEL_H)
        sheet.paste(cell, (x, y), cell)
        draw.text((x + 10, y + CROP_H + 9), name, fill=(235, 235, 235, 255))
    sheet_path = os.path.join(out_dir, "seraphine_expressions.png")
    sheet.save(sheet_path)
    print(f"PACK {sheet_path}")


if __name__ == "__main__":
    main()
