#!/usr/bin/env python3
"""Diagnostic: does the liveness model expect [0,1] or [0,255] inputs?

gaze/src/liveness.rs feeds the MiniFASNetV2 anti-spoofing model raw 0-255 BGR
values (see `pre_process`). The reference Silent-Face pipeline normalises with
`transforms.ToTensor()`, i.e. divides by 255 to get [0,1]. The exported ONNX
graph starts with a Conv directly on `input` (no baked-in /255 or mean/std), so
the model expects the [0,1] range it was trained on.

This script runs the model both ways on sample images and prints the class
softmax (class 1 == "live" in the Silent-Face convention) so the mismatch is
visible. It is a manual diagnostic, not part of the build or test suite.

Usage:
    pip install onnxruntime onnx numpy pillow
    python3 scripts/check-liveness-preprocessing.py [model.onnx] [image ...]

If no model is given it falls back to /var/cache/gaze/minifasnet_v2.onnx
(readable as root). Images default to whatever paths you pass; a real webcam
capture of a genuine face is the only input that proves the live-positive case.
"""

import sys

import numpy as np
import onnxruntime as ort
from PIL import Image

MODEL = sys.argv[1] if len(sys.argv) > 1 else "/var/cache/gaze/minifasnet_v2.onnx"
IMAGES = sys.argv[2:] or []


def run(sess, iname, img_path, scale_to_unit):
    im = Image.open(img_path).convert("RGB").resize((80, 80))
    arr = np.asarray(im).astype(np.float32)          # HWC, RGB, 0-255
    bgr = arr[:, :, ::-1]                            # match OpenCV BGR layout
    t = np.transpose(bgr, (2, 0, 1))[None, ...].copy()  # NCHW
    if scale_to_unit:
        t = t / 255.0
    logits = sess.run(None, {iname: t})[0][0]
    e = np.exp(logits - logits.max())
    probs = e / e.sum()
    return logits, probs


def main():
    sess = ort.InferenceSession(MODEL)
    iname = sess.get_inputs()[0].name
    print(f"model: {MODEL}")
    print(f"input: {sess.get_inputs()[0].shape}  output: {sess.get_outputs()[0].shape}")
    if not IMAGES:
        print("\nNo images supplied. Pass face crops, e.g.:")
        print("  python3 scripts/check-liveness-preprocessing.py model.onnx face.jpg")
        return
    for path in IMAGES:
        print(f"\n=== {path} ===")
        for unit, label in [
            (False, "raw 0-255  (current gaze pre_process)"),
            (True, "0-1 norm   (reference ToTensor)"),
        ]:
            logits, probs = run(sess, iname, path, unit)
            print(f"  {label}")
            print(f"    logits  = {np.round(logits, 3)}")
            print(f"    softmax = {np.round(probs, 4)}   live(class1) = {probs[1]:.4f}")


if __name__ == "__main__":
    main()
