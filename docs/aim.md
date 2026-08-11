Responsive Aiming & Attack Animation Design

Overview

This document describes a responsive aiming and attack animation system for a third-person action game using Panda3D and Blender.

The intended combat feel draws from games such as Earth Defense Force and Armored Core, where the player may move independently of the direction they are attacking, and different weapons can use different levels of aiming assistance.

The central animation approach is:

Use Blender-authored animations for the weight, anticipation, recoil, swing arcs, follow-through, and locomotion.

Separate upper-body combat animation from lower-body locomotion where useful.

Apply a limited amount of procedural runtime correction in Panda3D to make attacks respond to the current aim direction.

Use different correction rules for ranged and melee attacks.

Avoid fully procedural animation unless a specific weapon or mechanic requires it.

Goals

The system should support:

Aiming independently of movement.

Firing while walking, strafing, sprinting, or airborne.

Melee attacks that respond to target direction without looking unnaturally magnetic.

Weapon-specific aiming behavior.

Gradual character body turning when aim exceeds comfortable torso limits.

Reusable animation architecture across many weapon types.

Runtime control from Panda3D without requiring hundreds of unique animations.

Blender-authored motion remaining the primary source of visual quality.

Core Animation Philosophy

The final pose should be produced by combining several layers:

Blender-authored animation
        │
        ├── lower-body locomotion
        │
        └── upper-body attack / recoil
                    │
                    ▼
          runtime aim correction
          torso / shoulders / arms
                    │
                    ▼
             final character pose
                    │
                    ▼
          weapon / hitbox position

The authored animation answers:

How should this attack move?

The procedural layer answers:

In what direction should this attack currently be oriented?

This keeps attacks responsive without sacrificing animation quality.

Recommended Skeleton Structure

A useful Blender rig could resemble:

root
└── pelvis
    ├── thigh.L
    │   └── ...
    ├── thigh.R
    │   └── ...
    └── spine
        └── AIM_TORSO
            └── chest
                ├── shoulder.L
                │   └── arm.L
                │       └── hand.L
                └── shoulder.R
                    └── arm.R
                        └── hand.R

AIM_TORSO Bone

AIM_TORSO is a dedicated runtime pivot.

It should not contain the primary authored attack motion.

Instead:

AIM_TORSO
    │
    └── chest
         ├── rifle recoil animation
         ├── sword swing animation
         ├── reload animation
         └── idle animation

Panda3D can rotate AIM_TORSO at runtime while the animated bones beneath it continue playing their normal Blender animation.

This avoids overriding an important animated chest or spine bone directly.

Animation Layers

The character can be thought of as having several animation layers.

Lower Body

Responsible for:

idle

walk

run

sprint

strafe

jump

landing

dodge movement

Example:

INPUT
  │
  ▼
Locomotion State
  │
  ▼
run_forward
strafe_left
strafe_right
etc.
  │
  ▼
LEGS

Upper Body

Responsible for:

weapon idle

fire

recoil

reload

melee windup

melee swing

melee recovery

weapon-specific poses

Example:

upper body
    │
    ├── rifle_idle
    ├── rifle_fire
    ├── rifle_reload
    ├── sword_idle
    ├── sword_slash
    └── sword_heavy

Procedural Aim Layer

Responsible for:

yaw correction

pitch correction

target-facing correction

limited torso twisting

optional shoulder distribution

optional arm IK

Panda3D Partial-Body Animation

Panda3D can use Actor subparts to play animations on only part of a skeleton.

Conceptually:

actor.makeSubpart(
    "upper",
    includeJoints=["AIM_TORSO"]
)

actor.makeSubpart(
    "lower",
    includeJoints=["Root"],
    excludeJoints=["AIM_TORSO"]
)

actor.loop("run", partName="lower")
actor.loop("rifle_idle", partName="upper")

Exact joint lists will depend on the exported skeleton.

The important idea is:

LOWER BODY
locomotion animation

        +

UPPER BODY
combat animation

        +

AIM_TORSO
procedural correction

Runtime Aim Direction

The gameplay system should calculate a desired world-space aim direction.

That direction is then converted into character-local yaw and pitch.

aim_world_direction
        │
        ▼
convert to character-local direction
        │
        ├── yaw
        └── pitch
        │
        ▼
clamp to allowed torso range
        │
        ▼
AIM_TORSO rotation

Conceptually:

yaw = clamp(target_yaw, -60, 60)
pitch = clamp(target_pitch, -45, 60)

aim_torso.setHpr(yaw, pitch, 0)

The actual Panda3D axes will depend on Blender bone orientation.

The AIM_TORSO bone should therefore be oriented intentionally in Blender so its local axes are convenient for runtime rotation.

Distributed Aim Rotation

Do not necessarily rotate a single bone by the full aim amount.

A better-looking result can distribute rotation.

Example:

Desired yaw: 50 degrees

AIM_TORSO      30°
upper spine    12°
shoulders       8°

This can make the character feel less robotic.

A simple first implementation can use only AIM_TORSO.

Additional joints can be added later.

Torso Aim Limits

The upper body should only twist so far before the lower body begins turning.

Example:

                 FORWARD
                    ↑

        -60°        │        +60°
          \         │         /
           \        │        /
            [ CHARACTER ]

Suggested behavior:

0–20°     shoulders / mild torso twist

20–60°    stronger upper-body rotation

60°+      lower body begins rotating toward aim

This gives the character responsiveness while preserving believable body mechanics.

Ranged Weapon Aiming

Ranged weapons can allow continuous aim correction.

Example situation:

Player is strafing right.

Camera aims upward and left.

Legs continue strafing.

Upper body rotates toward the firing direction.

Rifle firing animation continues normally.

movement direction  ───────►

        [ character ]
             /
            /
           ↖ aim direction

The animation stack becomes:

strafe animation
      +
rifle fire animation
      +
torso aim correction
      =
final firing pose

Ranged Weapon Architecture

Player Camera / Targeting
          │
          ▼
Desired Aim Direction
          │
          ▼
Weapon-specific Aim Solver
          │
          ▼
AIM_TORSO correction
          │
          ▼
Weapon muzzle orientation
          │
          ▼
Projectile / hitscan

Different weapons can use different aim solvers.

Examples:

Rifle

moderate aim correction

fast torso response

low recoil commitment

Heavy Cannon

slower torso response

reduced turn speed while firing

strong recoil

possible movement penalty

Rocket Launcher

manual direction

low precision requirement

projectile travel time

Missile Launcher

lock-on targeting

upper body only needs to approximately face the target

projectile performs most of the final tracking

Weapon Sockets

Weapons should be attached to explicit skeleton sockets.

Example:

hand.R
  │
  └── WEAPON_SOCKET
          │
          └── weapon model
                  │
                  └── MUZZLE

Panda3D can expose a joint as a runtime NodePath.

Conceptually:

weapon_socket = actor.exposeJoint(
    None,
    "modelRoot",
    "WEAPON_SOCKET"
)

rifle.reparentTo(weapon_socket)

The weapon then follows the animated hand.

The weapon model can also contain its own muzzle node.

Two-Handed Weapons and IK

For a two-handed rifle, a useful long-term setup is:

right hand
    │
    ▼
primary weapon attachment
    │
    ▼
weapon
    │
    ├── muzzle
    │
    └── LEFT_HAND_GRIP
              ▲
              │
         left-hand IK

The right hand can drive the weapon.

The left hand can use IK to remain attached to a grip position.

Final pose:

locomotion
    +
upper-body gun animation
    +
torso aim
    +
weapon orientation
    +
left-hand IK

IK should be considered an enhancement rather than a requirement for the first prototype.

Melee Aiming

Melee should not use exactly the same correction rules as ranged combat.

Ranged aiming can remain responsive continuously.

Melee should become progressively more committed as the attack advances.

Suggested phases:

PRESS ATTACK
     │
     ▼
WINDUP
tracking = high
     │
     ▼
SWING START
tracking = medium
     │
     ▼
ACTIVE FRAMES
tracking = low
     │
     ▼
FOLLOW THROUGH
tracking = none

Example values:

Windup          100%
Early swing      60%
Active frames    15%
Follow-through    0%

This creates responsive attacks without allowing the weapon to unnaturally curve toward a dodging enemy.

Melee Design Principle

Do not make procedural IK generate the primary melee swing.

Instead:

BLENDER
  │
  ├── anticipation
  ├── shoulder motion
  ├── elbow motion
  ├── wrist rotation
  ├── weapon arc
  └── follow-through

Panda3D then modifies the larger attack orientation:

target direction
      │
      ▼
AIM_TORSO
      │
      ▼
authored sword animation

The system therefore means:

Perform this authored attack in approximately this direction.

Not:

Move the character's hand directly to the enemy.

Melee Tracking by Weapon

Different melee weapons can use different tracking characteristics.

Fast Sword

Windup tracking:       High
Swing tracking:        Medium
Active tracking:       Low
Lunge distance:        Medium
Turn rate:             High

Heavy Hammer

Windup tracking:       Medium
Swing tracking:        Very Low
Active tracking:       None
Lunge distance:        Low
Turn rate:             Low

Lance

Windup tracking:       High
Initial correction:    High
Active tracking:       Low
Forward movement:      High
Lateral correction:    Low

Pile Driver

Windup tracking:       Low
Active tracking:       None
Range:                 Short
Damage:                Very High
Commitment:            Very High

This makes aiming behavior part of weapon identity.

Melee Attack Timeline

A melee attack can expose animation-driven gameplay events.

Example:

0.00 ───────────── 0.25 ───────────── 0.55 ───────────── 0.90

WINDUP              ACTIVE              RECOVERY

tracking: 100%      tracking: 20%       tracking: 0%

                    hitbox ON

                                       hitbox OFF

Animation events can trigger:

hitbox activation

hitbox deactivation

movement impulse

lunge start

lunge stop

sound

visual effects

camera shake

tracking reduction

Melee Hit Detection

For melee, prefer following the animated weapon rather than approximating the entire attack with a single forward ray.

Possible methods:

Weapon Collider

Attach a collision shape to the animated weapon.

hand
  │
weapon
  │
blade collider

Swept Samples

Track several points along the weapon between frames.

previous frame

A-----B-----C

current frame

   A-----B-----C

Sweep between old and new positions to detect collisions.

This reduces tunneling during fast swings.

Aim Offsets

If direct procedural torso rotation does not provide enough animation quality, add authored aim poses.

Example 2D pose set:

UP-LEFT        UP        UP-RIGHT

LEFT          CENTER       RIGHT

DOWN-LEFT     DOWN      DOWN-RIGHT

The runtime system determines:

aim yaw
aim pitch

and blends between nearby authored poses.

This gives artists more control over:

shoulder placement

spine curvature

elbow shape

weapon silhouette

extreme aim angles

Aim offsets are an optional second-stage improvement.

Suggested Animation Set

Locomotion

idle
walk_forward
walk_backward
strafe_left
strafe_right
run_forward
run_backward
sprint
jump
fall
land

Flying

fly_takeoff
fly_hover
fly_forward
fly_backward
fly_strafe_left
fly_strafe_right
fly_ascend
fly_descend
fly_land

Skating

skate_start
skate_stride
skate_glide
skate_turn_left
skate_turn_right
skate_brake
skate_jump
skate_land
skate_stop

Rifle

rifle_idle
rifle_fire
rifle_reload
rifle_recoil_heavy

Heavy Weapon

heavy_idle
heavy_fire
heavy_recover

Sword

sword_idle
sword_slash_1
sword_slash_2
sword_heavy
sword_lunge

Additional attacks can be added as needed.

Blender Workflow

1. Build the Skeleton

Include:

root

pelvis

leg chains

spine

AIM_TORSO

chest

arms

hands

weapon sockets

2. Create Actions

Create separate Blender Actions for:

locomotion

flying

skating

ranged attack animations

melee attack animations

reloads

recoil

special attacks

3. Keep Runtime Pivot Clean

Avoid putting important authored rotation directly on AIM_TORSO.

Animate the child bones beneath it instead.

4. Export

Export the rig and actions using the Panda3D-compatible asset pipeline chosen for the project.

Verify:

bone names

bone orientations

action names

root transforms

scale

weapon socket transforms

Panda3D Runtime Architecture

Suggested systems:

CharacterController
      │
      ├── MovementController
      │
      ├── AnimationController
      │
      ├── AimController
      │
      ├── WeaponController
      │
      └── MeleeController

AimController

Responsibilities:

calculate local aim yaw

calculate local aim pitch

clamp torso rotation

smooth aim motion

control AIM_TORSO

request lower-body turning when torso limits are exceeded

Pseudo-interface:

class AimController:
    def set_aim_direction(self, world_direction):
        pass

    def update(self, dt):
        pass

    def get_local_yaw(self):
        pass

    def get_local_pitch(self):
        pass

AnimationController

Responsibilities:

locomotion state

upper-body animation state

animation transitions

animation subparts

attack animation playback

animation events

Example:

animation.play_lower("strafe_right")
animation.play_upper("rifle_fire")

WeaponController

Responsibilities:

equipped weapon

firing

muzzle transform

projectile spawning

weapon-specific aim behavior

recoil

ammunition

MeleeController

Responsibilities:

attack phase

target correction

lunge movement

hitbox activation

weapon sweep

tracking strength

attack commitment

Example:

tracking_strength = attack.get_tracking_strength(normalized_time)

Character Turning

The character should gradually rotate the lower body if the target moves beyond torso limits.

Example:

desired aim yaw = 85°

torso max = 60°

torso yaw = 60°

remaining = 25°

character body begins turning 25° toward aim

Pseudo-logic:

if abs(local_aim_yaw) > torso_limit:
    excess = abs(local_aim_yaw) - torso_limit
    rotate_character_toward_aim(excess, dt)

Rotation speed can depend on:

current movement state

equipped weapon

character stats

attack phase

airborne state

Smoothing

Avoid snapping the torso instantly.

Use a configurable response speed.

desired aim
     │
     ▼
smooth interpolation
     │
     ▼
current torso aim

Possible weapon-specific values:

Pistol       fast
Rifle        fast
Machine gun  medium
Cannon       slow
Launcher     slow
Sword        attack-phase dependent

This can become part of weapon feel.

Recommended First Prototype

Do not build the entire system at once.

Start with:

Blender

Skeleton:

root
pelvis
legs
spine
AIM_TORSO
chest
shoulders
arms
hands
WEAPON_SOCKET

Animations:

idle
run_forward
run_backward
strafe_left
strafe_right

rifle_idle
rifle_fire

sword_idle
sword_slash
sword_heavy

Panda3D

Implement:

Lower-body locomotion.

Upper-body ranged animation.

Runtime AIM_TORSO yaw and pitch.

Torso rotation limits.

Character body rotation beyond torso limits.

Weapon socket attachment.

Basic melee swing.

Melee tracking that decreases through the attack.

Prototype Architecture

                     CHARACTER
                         │
          ┌──────────────┴──────────────┐
          │                             │
          ▼                             ▼
      LOWER BODY                    UPPER BODY
    locomotion anim                attack anim
          │                             │
          │                        AIM_TORSO
          │                       procedural
          │                             │
          └──────────────┬──────────────┘
                         ▼
                     final pose
                         │
                         ▼
                 exposed weapon
                         │
              ┌──────────┴──────────┐
              ▼                     ▼
            muzzle              melee collider

Later Improvements

Once the basic system works, consider:

2D aim-offset poses.

Shoulder and spine rotation distribution.

Left-hand IK for rifles.

Foot IK on uneven terrain.

Weapon-specific turning limits.

Recoil affecting the actual aim solver.

Camera-relative versus target-relative attacks.

Lock-on assisted melee.

FCS-style ranged tracking.

Predictive projectile leading.

Procedural weapon stabilization.

Animation warping for lunges.

Root-motion melee attacks.

Combo-specific target correction.

Heavy weapon bracing poses.

Design Rule Summary

Use this hierarchy:

AUTHORED ANIMATION
determines motion quality

        +

PARTIAL-BODY ANIMATION
allows locomotion and attacks simultaneously

        +

PROCEDURAL AIM
makes the pose responsive

        +

IK / AIM OFFSETS
solve specific visual problems

For ranged combat:

Keep aim correction active and responsive.

For melee combat:

Allow strong correction during windup, then progressively reduce correction as the attack commits.

For both:

Let Blender create the motion. Let Panda3D decide where that motion should be directed.

Primary Recommendation

For the first implementation, use:

Blender-authored attacks + separate lower-body locomotion + a procedural AIM_TORSO bone in Panda3D.

This approach is simple enough to prototype but flexible enough to support:

rifles

machine guns

rocket launchers

cannons

swords

lances

hammers

punches

pile drivers

lock-on weapons

aim-assisted weapons

It also leaves room to add IK, aim offsets, FCS behavior, and more advanced procedural animation later without rebuilding the core animation architecture.
