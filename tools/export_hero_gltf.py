"""Export the Hero and his clips out of Blender as glTF.

Run inside Blender (The Hero.blend on the Windows host), the same way
build_valkyrie.py is run -- there is no decomp data behind this actor, so
tools/export_actor_gltf.py has nothing to work from and this stands in its
place.

Three things about the file make a plain "File > Export" wrong:

**The rig ships in rest position.** `rig.data.pose_position` is REST, which
makes the armature ignore pose evaluation entirely -- the clips still hold
their keyframes, and every exported animation comes out as the bind pose held
still. Forced to POSE here rather than left to whoever exports next.

**Rigify carries 240 bones, 53 of which deform.** The rest are controls,
mechanisms and widget targets that mean nothing outside Blender.
`export_def_bones` drops them, which is also what keeps the exported skeleton
close enough to Mario's 30 joints to be worth animating at runtime.

**The eyes carry their own action.** `Eyes` has `jump up` assigned to it
independently of the rig, so an ACTIONS-mode export emits a second, unrelated
animation under a name the rig also uses. It is unassigned for the duration of
the export and put back afterwards.

The clips come out at Blender's scale (he is 1.18 units tall); the game scales
him at runtime, where one constant is easier to tune than a re-export.

Usage, from the MCP connection or Blender's text editor:

    exec(open("//wsl.localhost/Ubuntu/home/bob/mario/tools/export_hero_gltf.py").read())

then, on the WSL side:

    python3 tools/adopt_blender_export.py assets/hero/hero_raw.glb \\
        --out assets/hero/hero.glb --sidecar assets/hero/hero_clips.json \\
        --skeleton-root rig

Or let tools/build_hero.py do both halves headless, which is the same thing
without the Windows round trip:

    python3 tools/build_hero.py
"""

import argparse
import sys

import bpy

RIG = "rig"

# Written where the repo can see it. Blender is on the Windows host and the
# checkout is in WSL, so this is a UNC path rather than a drive path.
OUT = r"\\wsl.localhost\Ubuntu\home\bob\mario\assets\hero\hero_raw.glb"


def selected_for_export(rig):
    """The rig and the meshes bound to it, and nothing else.

    The widget collections are excluded from the view layer already, but
    exporting by selection rather than by visibility keeps that from being the
    thing the export depends on.
    """
    bpy.ops.object.select_all(action="DESELECT")
    rig.select_set(True)
    meshes = [o for o in bpy.data.objects
              if o.type == "MESH" and o.parent == rig]
    for mesh in meshes:
        mesh.select_set(True)
    bpy.context.view_layer.objects.active = rig
    return meshes


def export(path=OUT):
    rig = bpy.data.objects[RIG]

    # Object mode: the exporter reads evaluated data, and pose or edit mode
    # leaves parts of it stale.
    if bpy.context.object and bpy.context.object.mode != "OBJECT":
        bpy.ops.object.mode_set(mode="OBJECT")

    was_rest = rig.data.pose_position
    rig.data.pose_position = "POSE"

    eyes = bpy.data.objects.get("Eyes")
    eyes_action = None
    if eyes and eyes.animation_data:
        eyes_action = eyes.animation_data.action
        eyes.animation_data.action = None

    meshes = selected_for_export(rig)
    try:
        options = dict(
            filepath=path,
            export_format="GLB",
            use_selection=True,
            export_yup=True,
            export_skins=True,
            export_def_bones=True,
            export_rest_position_armature=True,
            export_animations=True,
            export_animation_mode="ACTIONS",
            # Each clip starts at t=0 rather than at whatever frame it was
            # authored on, so the sidecar's start_frame stays 0 throughout.
            export_anim_slide_to_zero=True,
            # Keyframe reduction makes the frame count depend on the curve
            # rather than on the clip, and the game reads frame counts to
            # drive action timing.
            export_optimize_animation_size=False,
        )
        # Do not use Blender 5.2's export_armature_object_remove here. Rigify's
        # DEF bones are constraint-driven and intentionally flat; removing the
        # rig object makes Blender emit the meshes without any skin at all.
        # adopt_blender_export normalizes this required root after export.
        bpy.ops.export_scene.gltf(**options)
    finally:
        rig.data.pose_position = was_rest
        if eyes is not None and eyes_action is not None:
            eyes.animation_data.action = eyes_action

    return {
        "path": path,
        "meshes": len(meshes),
        "deform_bones": sum(1 for b in rig.data.bones if b.use_deform),
        "actions": len(bpy.data.actions),
    }


if __name__ == "__main__":
    # Under `blender --background ... --python this_file -- --out X` the
    # arguments arrive after a bare `--`. Pasted into Blender's text editor
    # there are none, and OUT (the Windows-side UNC path) still stands.
    argv = sys.argv[sys.argv.index("--") + 1:] if "--" in sys.argv else []
    ap = argparse.ArgumentParser(prog="export_hero_gltf")
    ap.add_argument("--out", default=OUT)
    print(export(ap.parse_args(argv).out))
