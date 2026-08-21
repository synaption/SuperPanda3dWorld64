#!/usr/bin/env python3
"""Convert an imported SM64 Blender scene to normal Blender authoring data.

Animated models keep a real Blender rig, but its imported object/data/action
names are normalized. Static models have their evaluated deformation baked
into the mesh and lose the pointless one-bone armature entirely.

    blender-5.2.0-linux-x64/blender --background -noaudio asset.blend \
        --python tools/convert_legacy_blend.py
"""

import bpy


def normalize_action_names():
    renamed = []
    suffix = "_Armature"
    for action in bpy.data.actions:
        if action.name.endswith(suffix):
            old = action.name
            action.name = action.name[:-len(suffix)]
            renamed.append((old, action.name))
    return renamed


def remove_static_armature(armature):
    affected = []
    for obj in list(bpy.context.scene.objects):
        modifiers = [modifier for modifier in obj.modifiers
                     if modifier.type == "ARMATURE"
                     and modifier.object == armature]
        for modifier in modifiers:
            bpy.ops.object.select_all(action="DESELECT")
            obj.select_set(True)
            bpy.context.view_layer.objects.active = obj
            bpy.ops.object.modifier_apply(modifier=modifier.name)
        if obj.parent == armature:
            world = obj.matrix_world.copy()
            obj.parent = None
            obj.matrix_world = world
        if modifiers:
            affected.append(obj.name)
    bpy.data.objects.remove(armature, do_unlink=True)
    return affected


def main():
    renamed = normalize_action_names()
    armatures = [obj for obj in bpy.context.scene.objects
                 if obj.type == "ARMATURE"]
    if len(armatures) != 1:
        raise SystemExit(f"expected one armature, found {len(armatures)}")
    armature = armatures[0]

    if bpy.data.actions:
        armature.name = "Rig"
        armature.data.name = "Rig"
        armature.show_in_front = True
        armature.data.display_type = "STICK"
        print(f"kept standard animated rig with {len(armature.data.bones)} bones")
    elif len(armature.data.bones) == 1:
        affected = remove_static_armature(armature)
        print(f"removed static one-bone armature from {affected}")
    else:
        raise SystemExit("refusing to remove a multi-bone rig without animations")

    for old, new in renamed:
        print(f"renamed action {old!r} to {new!r}")
    bpy.ops.wm.save_as_mainfile(filepath=bpy.data.filepath, compress=True)


if __name__ == "__main__":
    main()
