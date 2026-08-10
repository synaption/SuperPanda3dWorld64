"""A Panda3D port of the Outer Wilds-style player controller.

Ported from the blueprint-only Unreal project in
`reference/OuterWildsPlayerControlle`. Module map, against the originals:

    constants.py   DA_CharacterVariables, AC_GravityComponent defaults
    gravity.py     AC_GravityComponent
    movement.py    AC_SpaceMovementComponent, BFL_ZeroGFunctions
    level.py       L_DemoLevel
    world.py       the tick that BP_PlayerController drove
    app.py         BP_Player plus the Unreal scene and Enhanced Input setup
"""

__all__ = ["constants", "gravity", "movement", "level", "world"]
