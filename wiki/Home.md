# Prism — User Guide

*Prism* (the Clay Engine demo, **prism-alpha**) opens on the **Main Menu**, titled
*Prism · The Seven Shards*. This page lists every mode and the scenes in each.

> **Living page.** Modes marked *under construction* have no scenes yet — the list grows
> as benches are added. When this moves to the wiki, each mode below can become its own page.

## The Main Menu

The menu is a two-column workbench:

- **Left — the mode menu.** The four play modes, plus Settings and Quit. Selecting a mode
  fills the right column with that mode's scenes.
- **Right — the scene selector.** The scenes belonging to the selected mode. Each is a row
  with a short description and a **LOAD** button. Click the row (or **LOAD**) to launch it.
  Before you pick a mode it reads *Select a Scene*.

Drive the menu with the **mouse or a controller** — a controller's Confirm launches the
focused button. Inside a running scene, press **ESC** to open the pause menu.

### Menu buttons (left column)

| Button | What it does |
|---|---|
| **ADVENTURER** | Shows the Adventurer scenes — the player-facing experiences. |
| **DUNGEON MAKER** | Build-the-world mode — *under construction*, no scenes yet. |
| **GAME MASTER** | Shows the Game Master scenes — authoring the world itself. |
| **DEVELOPER MODE** | Shows the Developer benches, tools, and reference scenes. |
| **SETTINGS** | Opens the settings panel. |
| **QUIT** | Exits the application. |
| **UPDATE AVAILABLE** | Appears only when a newer release exists; opens the update page. |

## Modes & scenes

### Adventurer
The player-facing experiences.

- **Solar Birth** *(Cinematic)* — A camera fly-in over the fixed Prism system as the dust
  clears. Drag to orbit, wheel to zoom, **Space** to replay the flight.
- **Click Trainer** *(Trainer)* — Click the shrinking targets before they time out. A 2D
  sprite game with live accuracy and reaction-time readouts.
- **Cluster Demo** *(Sandbox)* — A voxel-carving playground: a 3×3 field of clusters you
  sculpt with CSG, drawn with a wireframe boundary and toggleable debug meshes. Fly the
  camera with WASD; R / F to rise and descend.

### Dungeon Maker
Build-the-world mode. *Under construction — no scenes yet.*

### Game Master
Authoring the world itself.

- **Populous Bench** *(Authoring)* — A hex map of a world: every tile its own index, with a
  dial from 23k to 144k tiles. Early days — nothing else yet.
- **World Builder** *(Epoch simulation)* — Watch a planet build itself forward through
  epochs on a globe you can orbit. Scrub the epochs, tweak the levers, and reseed, with a
  live readout and life-supporting gauges.

### Developer
Engine benches, tools, and reference scenes.

- **Component Catalog** *(Reference)* — One live copy of every UI widget with all features
  on: a nav rail of bookmarks over a card per control. The UI test scene.
- **Quartermaster Bench** *(Content manager)* — The content air-traffic controller: review
  what landed in staging and promote it into the package, then rearrange the trees.
  Move-only; every change is a single undo.
- **Sablework Bench** *(Texture synth)* — The texture synthesizer console: six noise voices
  mixed into a tiled swatch, an output stage, and a lit turntable preview. **Commit** bakes
  the material into staging.
- **Clayworks Bench** *(Asset pipeline)* — The asset-pipeline editor: a step-by-step wizard
  that ingests external props, garments, and rigs, previews the retargeted animation clip,
  and **Commit**s the result into staging.
- **Loomforge Bench** *(Animation)* — The animation-authoring editor: a four-tab bench —
  State Machine, Pack Browser, Creature Composer, and TAE Editor — that edits a creature
  pack and writes it back.

## The pause menu

Press **ESC** in any scene to open the pause menu (**SANCTUM**); press **ESC** again to return.

| Button | What it does |
|---|---|
| **RETURN TO WORLD** | Closes the pause menu and resumes the scene. |
| **SETTINGS** | Opens the settings panel. |
| **MAIN MENU** | Leaves the scene and returns to the Main Menu. |
| **QUIT** | Exits the application. |

*Settings, Main Menu, and Quit show when they apply to the current scene; Return to World is always present.*
