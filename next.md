
multithreading?
jumping doesn't always work
fall through floor sometimes
amortize/SoA

enemies should use pathing, flow fields, and group based calculation.  
guns for mario
bigger hit box/valid attack area

is it possible for some items to not be vertex shaded?  

vars in the console should spawn sliders like the panda3d did
get rid of everything that pulls assets from sm64.  Just keep the assets that have already been pulled and not the code to pull them.  

camera/aim/player pose adjustment
- like overwatch
- the players torso should be able to twist up to 20 degrees in the direction of aim before the legs start moving along too
- need sideways movement animation
- head should also look where aiming

smooth camera/aim without causing lag like overwatch

need vfx energy/aura/trails on weapons


cool centipied enemy that jumps in and out of the ground like LTTP sand worm


X pylons! should share logic with pathing.  traveling salemen problem, creeper world
X place pylons like stellarator, connect to network with beems along LOS.  

grid2quad system

make ow into a level


save file format, including settings

SoA amortization

ctrl+c/ctrl+v in ingame console

pop in

marios stuck in corner, need to be smart enough not to get stuck, pathing?  


Planet gen topology needs to be walkable and flat enough to farm.  


clone technology?



m16/granades/rocket launcher/shield/turrets

bevy plugins
- bevy_hanabi VFX
- bevy_tweening 
- bevy_asset_loader 
- bevy_kira_audio 
- bevy_egui 
A crowd benchmark control panel.  
Flow-field visualization controls.  
Impostor bake previews.  
Live graphs of simulated/model/impostor enemy counts.  
Frame-spike visualization.  
A level and furniture debugging panel.  
Animation-clip and aim-rig inspection.  
- avian3d
- leafwing-input-manager , input should not latch at render rate.  


dash

X rename hero to Luna, rename game to Space Crusaders, organize all assets.  

X make luna AI playable too

furniture::tests::the_castle_is_where_it_always_was fails: the ant warp pipe moved from x = 46.8 to x = 53.45 in assets/bevy/castle_furniture.json..  this move is intentional, but it should not require farther changes anywhere else in the codebase.  blender is the source of truth for scenes.  things should be able to move in the castle grounds blend file.  Also the actual modles themselves should be used, like the warp pipe should ba the actual warp pipe, and not a box.  same with the stellarator, player start point, and initial units.  most things should be placed in the scene like this.  

X reorganize, clear clutter, get rid of unneccisary tests

X I do not want to preserve  SM64 collision quirks, including triangle orderingand unusual wall-resolution behavior

sometimes I have CPU load spikes per frame until I change the frame rate limit,but If I change it back to the same limit the spikes go away. 

health bar should be blue

enemies are getting inside walls a lot, I still fall through things often

need a way to spawn mario

portals

VRR support?

orbs need to obey gravity and fall if an enemy dies on a wall for instance.  

marios have a hard time punching things that are farther away.  enemies should generally prefer to target other marios that are targeting them rather than chasing after units that have a different goal.  

vfx needs lightning/electricity

environmental clouds (prerendered asthetic), lightning, fog, better ground textures.  bumpmaps.  

tree shadows clip, but unit shadows never do.  

nuclonium orbs in the stellarator should have an individual slight modifier to their speed of +- 1%

enemies need the A* too.  this is why I asked for groupings, so it could handle a lot of units at once.  