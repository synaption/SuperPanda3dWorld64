
multithreading?
jumping doesn't always work
fall through floor sometimes
amortize/SoA


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

pop in shadows random delay



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


X make luna AI playable too


X reorganize, clear clutter, get rid of unneccisary tests

X I do not want to preserve  SM64 collision quirks, including triangle orderingand unusual wall-resolution behavior

sometimes I have CPU load spikes per frame until I change the frame rate limit,but If I change it back to the same limit the spikes go away. this does not happen with vsync off.

health bar should be blue

enemies are getting inside walls a lot, I still fall through things often

need a way to spawn mario


VRR support?

orbs need to obey gravity and fall if an enemy dies on a wall for instance.  

marios have a hard time punching things that are farther away.  enemies should generally prefer to target other marios that are targeting them rather than chasing after units that have a different goal.  

vfx needs lightning/electricity

environmental clouds (prerendered asthetic), lightning, fog, better ground textures.  bumpmaps.  

tree shadows clip, but unit shadows never do.  

make the coper material on the stellarator better

asteroid generation


I want the closes 100 units to be fully simulated instead of impostors, even if they are far away.  For disance based impostors, I don;t want them to change right away. if they enter or leave the range I want there to be a certain probability that they change based on how close they are every cycle to prevent suden pop in along the line as I move around.  