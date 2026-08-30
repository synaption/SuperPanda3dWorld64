
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

marios have a similar stutter when they are following luna.  the ball trail still starts so abruptly.  I don't like the visibale transition.  it should be a smooth transition that wraps around the ball.  the balls should not abrupty change location.  they should always smoothly transition.  for instance when they go into a pylon they should not abruptly aprear in the pylon network.  they should physically travel from where they are to the network.  the orbs need to be emissive.  they should liket their suroundings slightly

vfx needs lightning/electricity