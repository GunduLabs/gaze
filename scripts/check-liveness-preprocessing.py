#!/usr/bin/env python3
"""Diagnostic: which preprocessing does the liveness model actually expect?

gaze/src/liveness.rs feeds the MiniFASNetV2 anti-spoofing model an 80x80 face
crop. The model is sensitive to BOTH channel order and value range, and getting
either wrong collapses the "live" score for a genuine face. Empirically (tested
against real webcam captures), the correct preprocessing is **RGB channel order
in the raw 0-255 range** — not BGR, and not [0,1]-normalized:

    order  range    live(class1) on a real face
    BGR    0-255    ~0.33   (original code, false-rejects)
    BGR    0-1      ~0.005  (normalized, worse)
    RGB    0-1      ~0.005
    RGB    0-255    ~0.90   <-- correct; 2D photos stay below ~0.66

This script runs all four combinations on supplied images so the difference is
visible. It detects+crops the face the same way the daemon does (2.7x bbox ->
80x80). Pass real webcam captures of a genuine face AND 2D photos (print/replay
proxies) to confirm live faces clear the 0.8 gate while photos do not. Manual
diagnostic only; not part of the build or test suite.

Usage:
    pip install onnxruntime numpy opencv-python-headless
    python3 scripts/check-liveness-preprocessing.py [model.onnx] image [image ...]
"""

import sys

import cv2
import numpy as np
import onnxruntime as ort

MODEL = sys.argv[1] if len(sys.argv) > 1 else "/var/cache/gaze/minifasnet_v2.onnx"
IMAGES = sys.argv[2:]
CROP_SCALE = 2.7

_cascade = cv2.CascadeClassifier(
    cv2.data.haarcascades + "haarcascade_frontalface_default.xml"
)


def crop_face(img_bgr):
    """Replicate crop_face() from liveness.rs: 2.7x bbox, clamped, resized 80x80."""
    gray = cv2.cvtColor(img_bgr, cv2.COLOR_BGR2GRAY)
    faces = _cascade.detectMultiScale(gray, 1.1, 5)
    if len(faces) == 0:
        return None
    x, y, w, h = sorted(faces, key=lambda f: f[2] * f[3])[-1]
    x1, y1, x2, y2 = x, y, x + w, y + h
    H, W = img_bgr.shape[:2]
    fw, fh = x2 - x1, y2 - y1
    scale = min(CROP_SCALE, (H - 1) / fh, (W - 1) / fw)
    sw, sh = fw * scale, fh * scale
    cx, cy = x1 + fw / 2, y1 + fh / 2
    left, top, right, bot = cx - sw / 2, cy - sh / 2, cx + sw / 2, cy + sh / 2
    if left < 0:
        right -= left; left = 0
    if top < 0:
        bot -= top; top = 0
    if right > W - 1:
        left -= right - W + 1; right = W - 1
    if bot > H - 1:
        top -= bot - H + 1; bot = H - 1
    left, top, right, bot = (int(max(v, 0)) for v in (left, top, right, bot))
    return cv2.resize(img_bgr[top : bot + 1, left : right + 1], (80, 80))


def softmax(x):
    e = np.exp(x - x.max())
    return e / e.sum()


def main():
    if not IMAGES:
        print(__doc__)
        return
    sess = ort.InferenceSession(MODEL)
    iname = sess.get_inputs()[0].name
    print(f"model: {MODEL}\n")
    print("                              class0   class1(live)  class2")
    for path in IMAGES:
        img = cv2.imread(path)  # BGR
        if img is None:
            print(f"{path}: cannot read")
            continue
        crop = crop_face(img)
        if crop is None:
            print(f"{path}: no face detected")
            continue
        print(path)
        for order in ("BGR", "RGB"):
            for unit, rng in ((False, "0-255"), (True, "0-1  ")):
                im = crop[:, :, ::-1] if order == "RGB" else crop  # crop is BGR
                t = np.transpose(im.astype(np.float32), (2, 0, 1))[None, ...].copy()
                if unit:
                    t /= 255.0
                p = softmax(sess.run(None, {iname: t})[0][0])
                star = "  <- daemon uses RGB/0-255" if (order, unit) == ("RGB", False) else ""
                print(f"    {order} {rng}              {p[0]:.4f}    {p[1]:.4f}      {p[2]:.4f}{star}")


if __name__ == "__main__":
    main()
