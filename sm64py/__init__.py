"""A Panda3D reimplementation of Super Mario 64's movement and collision.

Layout:
    math_util  -- binary-angle trig and Panda3D coordinate bridging
    surfaces   -- static collision triangles, spatial partition, queries
    mario      -- Mario's state, physics stepping, and action state machine
"""

__all__ = ["math_util", "surfaces", "mario"]
