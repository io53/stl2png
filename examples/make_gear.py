#!/usr/bin/env python3
"""Generate examples/gear.stl — the demo model used in the README. No dependencies."""
import math
import struct

TEETH, R_OUT, R_IN, R_BORE, H, R_HUB, H_HUB = 16, 20.0, 16.5, 5.0, 6.0, 9.0, 10.0
N = TEETH * 8


def gear_profile():
    pts = []
    for i in range(N):
        a = 2 * math.pi * i / N
        t = i % 8
        r = R_OUT if t in (1, 2, 3) else (R_IN if t in (5, 6, 7) else (R_OUT + R_IN) / 2)
        pts.append((r * math.cos(a), r * math.sin(a)))
    return pts


def circle(r):
    return [(r * math.cos(2 * math.pi * i / N), r * math.sin(2 * math.pi * i / N)) for i in range(N)]


tris = []


def quad(a, b, c, d):
    tris.append((a, b, c))
    tris.append((a, c, d))


def ring(outer, inner, z0, z1):
    for i in range(N):
        j = (i + 1) % N
        o0, o1, i0, i1 = outer[i], outer[j], inner[i], inner[j]
        quad((*o0, z1), (*o1, z1), (*i1, z1), (*i0, z1))  # top
        quad((*o0, z0), (*i0, z0), (*i1, z0), (*o1, z0))  # bottom
        quad((*o0, z0), (*o1, z0), (*o1, z1), (*o0, z1))  # outer wall
        quad((*i0, z0), (*i0, z1), (*i1, z1), (*i1, z0))  # inner wall


ring(gear_profile(), circle(R_HUB), 0, H)
ring(circle(R_HUB), circle(R_BORE), 0, H_HUB)

with open("examples/gear.stl", "wb") as f:
    f.write(b"stl2png demo gear".ljust(80, b"\0"))
    f.write(struct.pack("<I", len(tris)))
    for tri in tris:
        f.write(struct.pack("<3f", 0, 0, 0))  # normal (recomputed by stl2png anyway)
        for p in tri:
            f.write(struct.pack("<3f", *p))
        f.write(b"\0\0")
print(f"wrote examples/gear.stl ({len(tris)} triangles)")
