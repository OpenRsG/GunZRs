# GunZ: The Duel (Steam, app 3139440) data formats

Recovered from the retail data files by structural analysis. `Gunz.exe`
(sha256 `a076a5d8…6035`, PE32 x86, image base `0x400000`) is Themida-packed:
section `.themida`, entry RVA `0x4c8b058` in `.boot` decompiles to an unpacking
stub (REA evidence under `.local/re/a076a5d8…/`). Game code is not statically
readable, so everything below is derived from data and checked against it.

Status labels: **observed** = verified on every retail file; **inferred** =
consistent with the data but not proven.

## MRS archive (`*.mrs`)

ZIP archive with obscured metadata. **Observed** on all 12 archives (8 543
entries, all CRC-32 checks pass via `mrs extract`).

- Local file header (30 bytes) and the file name after it are each XORed with
  the same keystream `K`, restarting at `K[0]` for each buffer. Decrypted, they
  are standard ZIP local headers (`PK\3\4`, version 20 or 10, flags 0, method
  0 or 8, no data descriptors, no extra field).
- Payloads are unobscured stored/raw-deflate data.
- The central directory is one buffer XORed with the same keystream from
  `K[0]` onward; entries carry a 36-byte NTFS timestamp extra field. The
  keystream has no period below 8 192 bytes (**observed**), so its generator is
  unknown. Readers chain local headers instead: from offset 0 each entry ends at
  `30 + name_len + compressed_size`; the first header that does not decrypt to
  `PK\3\4` starts the central directory.
- End record: 22 bytes, XORed from `K[0]`; signature decrypts to `0x05030208`
  (not `PK\5\6`), the remaining fields are standard.
- `K[0..256]` is in `src/mrs.rs`. It was recovered as known plaintext from the
  central directories (fields duplicated from the local headers), majority-voted
  across all archives. `K[83..91]` comes from NTFS timestamp high bytes; `K[84]`
  and `K[87]` have a single vote each. The longest retail name is 65 bytes.

Virtual paths: `<archive path without .mrs>/<entry name>`, case-insensitive,
e.g. `Maps.mrs` + `Mansion/Mansion.RS` → `maps/mansion/mansion.rs`. No
collisions across the install.

## Map (`maps/<name>/<name>.RS*`)

All 31 retail maps use RS version 7. Little-endian, left-handed, Z up.

### `.RS` (**observed**: parse consumes Mansion exactly and matches every count)

```
u32 id = 0x12345678, u32 version = 7
i32 material_count; material_count × NUL-terminated name   (same order as the XML MATERIALLIST)
i32 convex_count, i32 convex_vertex_total
convex_count × { i32 material, u32 flags, f32 plane[4], f32 area, i32 n, f32 pos[n][3], f32 normal[n][3] }
i32 bsp_nodes, bsp_polygons, bsp_vertices, bsp_indices      (tree stored in .RS.bsp)
i32 oct_nodes, oct_polygons, oct_vertices, oct_indices
node (recursive, pre-order):
  f32 bbox[6], f32 plane[4], u8 has_positive, [node], u8 has_negative, [node]
  i32 polygon_count
  polygon_count × { i32 material, i32 convex_index, u32 flags, i32 n,
                    n × { f32 pos[3], f32 normal[3], f32 uv[2], f32 unused[2] },
                    f32 normal[3] }
```

Render polygons are convex fans; `oct_indices == Σ 3(n-2)`. All 30 retail
`.RS` files pass the loader's exact-size and count checks.
Polygon `material` indexes the RS material list directly; `-1` (Factory,
Halloween Town, Island) means no material.

### `.RS.lm` (**observed** on Mansion)

```
u32 id = 0x30671804, u32 version = 3, i32 convex_count (must match .RS), i32 node_count
i32 lightmap_count; lightmap_count × { i32 size, size bytes of BMP (24-bit, 1024²) }
i32 order[oct_polygons]          permutation of octree polygon indices
i32 lightmap_index[oct_polygons] lightmap_index[k] belongs to octree polygon order[k]
f32 uv[oct_vertices][2]          octree vertex file order
```

Order checks (Mansion, 3 atlases): pieces of one convex polygon share an exact
affine position→lightmap-UV map only when `uv` follows octree file order
(908/908 groups, max error 8e-9; via `order` or its inverse, 1.2 %). For
`lightmap_index`, reading through `order` gives every split convex polygon one
atlas (0 conflicts vs 301 in file order) and cuts overlapping atlas texels from
11.8 % to 0.16 %. File order made blocks of geometry sample the wrong atlas.

A map may lack `.RS.lm`: `challengequest/maps/2/2.rs` (a developer test plane: its `belog.txt` shows a BSP export
only, no lighting pass) loads with no lightmaps and is drawn with diffuse only (white lightmap, scale 1).
Quest maps (`quest/maps`) and challenge-quest maps (`challengequest/maps`, except `2`) all have one. Of the 88
`.rs` files in the install, 86 load; `challengequest/maps/g_easy_2/mansion_hall2.rs` and
`g_easy_3/mansion_hall2.rs` are stray exports (different bytes from the real `mansion_hall2`) without `.xml`
and `.lm`, referenced by nothing (`scenario2.xml` uses `G_Easy_2`/`G_Easy_3`, i.e. their own `g_easy_N.rs`) and
are not loadable. `map::find_rs` finds maps in all three roots.

### `.RS.xml`

`MATERIALLIST/MATERIAL[@name]` with `DIFFUSEMAP` and optional empty flag tags
`USEOPACITY`, `USEALPHATEST`, `ADDITIVE`, `TWOSIDED`. `DIFFUSEMAP` is relative to
the map directory and may climb out of it (`../Dungeon/x.dds`); `.bmp`
references usually have a `.dds` twin, which is what we load. Two retail references dangle, both on materials
that no polygon uses (**observed**, 0 polygons): Ruin `gzd_map_ruin_moon_fx00` (`gzd_map_Ruin_moon.dds`; the
file present is `gzd_map_ruin_moon_fx00.dds`) and Snow Town `hide` (`../town/`, a directory); the loader only
loads the textures of materials some polygon uses. 9 more used materials have no `DIFFUSEMAP` (a `MATERIAL` block
without it, or the unnamed `-1` material; Factory 22 polygons, Garden 73, Halloween Town 3, Island 8, Port 11, Ruin 54)
and are drawn untextured.
`DUMMYLIST/DUMMY[@name]` with `POSITION`/`DIRECTION` (spawns are `spawn_*`).
Also `LIGHTLIST`, `OBJECTLIST` (`.elu` props, below),
`OCCLUSIONLIST`, `AMBIENTSOUNDLIST`.

### `.RS.col` (`src/col.rs`; **observed**: all 30 retail files parse to exactly their size)

Solid-leaf BSP used for collision. Little-endian, same space as `.RS` (cm, Z up).

```
u32 id = 0x5050178f, u32 version = 0, u32 node_count, u32 face_count
node (recursive, pre-order):
  f32 plane[4]            unit normal + d, positive side where n·p + d >= 0
  u8 solid                (1 only on leaves; internal nodes always 0)
  u8 has_positive, [node]
  u8 has_negative, [node]
  u32 face_count          (0 on internal nodes; every leaf has faces)
  face_count × { f32 v[3][3], f32 normal[3] }      triangle + unit normal, 48 bytes
```

Mansion: 22 234 nodes (4 040 leaves, 14 150 of the inner nodes have only a negative child),
38 251 faces, depth 39; other maps depth ≤ 54. Counts in the header match the tree.

Semantics (**observed**, Mansion):
- `solid` is the cell's contents. Probes 5–150 cm above the 71 spawn points land in `solid = 0`
  leaves (70/71 at +5 cm, 71/71 at +100 cm); probes 20 cm below land in `solid = 1` leaves
  (69/71). Walking a ray from a spawn to the first `.RS` hit, the air→solid change falls at a
  median 3.5 cm before the render surface on walls and ceilings and 0 cm on floors.
- Leaf faces lie on the leaf's bounding planes (all but 171 of 38 251 stay inside the leaf's
  half-spaces within 0.5 cm), i.e. each leaf lists the faces of its convex cell. Faces of air
  leaves are real surfaces (12 714 of 12 732 have air on the normal side and solid behind).
  Solid leaves list 25 519: 7 209 are surfaces (normal towards air), 695 are surfaces whose
  stored normal points into the solid, 17 608 are internal cell boundaries (solid both sides).
  `col.rs` keeps exactly the faces with air on one side (probing ±0.5 cm along the normal),
  oriented towards the air: 20 614 triangles for Mansion.
- Stored normal is the clockwise-winding normal in GunZ space (right-handed cross product of
  `(v1-v0, v2-v0)` is `-normal` for every face).
- A child missing from an inner node is treated as solid; 32 of 76 502 probe points reached one.
- No per-polygon flags: ladders, passthrough or material data are not in `.col`. Nothing other
  than `solid` is stored per cell.

Relation to the render mesh (**observed**): same coordinate frame (bounding boxes within
0.3 %), but it is a separate mesh: no triangle and 0.4 % of vertices coincide with `.RS`
polygons. Floors coincide (spawn down-rays below), walls are close but not identical, and
the collision mesh has extra blockers (windows, rails, invisible walls). Over 45 000 random rays
from spawn points (30 maps, 1 500 each): first hit within 10 cm of the first `.RS` render hit
for 68.2 %, within 2 cm for 49.4 %; col hit more than 10 cm earlier for 23 %, later for 3.3 %.

Not decoded yet: `.RS.bsp` (id `0x35849298` v2, collision/picking tree, not needed since
`.col` suffices), `.elu` models, `.elu.ani` animations.

## Shading (**inferred**)

Texture × lightmap × 4 in gamma space, clamped (D3D `MODULATE4X`-style), then
converted to linear for the sRGB target (`src/map.wgsl`). Lightmap texels have
a median of ~0.13; ×4 matches the game's look while ×2 is clearly too dark. The
packed executable could not confirm the texture-stage setup.
`ADDITIVE` materials (light shafts like Mansion `gzd_map_Mansion_fx00_2s_add`,
fire, water glints) are drawn unlit as `src + dst`; their DXT1 textures have no
alpha, so opaque drawing shows them as black slabs.

## Animation (`*.elu.ani`; `src/ani.rs`, `src/anim.rs`)

**Observed**: `ani::load` parses all 1 891 retail `.ani` exactly (every byte
consumed, no trailing data): model 1 381, sfx 134, maps 181, interface 140,
quest 55. By (version, kind): 0x1003 bone 1154 / transform 443 / vertex 10;
0x1001 bone 238 / transform 24 / vertex 1; 0x12 bone 21 (all `model/lo`).
Little-endian, all matrices D3D row-major row-vector (translation in row 3), same
space as ELU (left-handed, Y up, centimetres).

```
u32 magic = 0x0107F060, u32 version (0x12 | 0x1001 | 0x1003)
u32 max_frame            index of the last frame (not a count)
u32 node_count, u32 kind (1 vertex, 2 bone, 3 transform)
node_count × {
  char name[40]          NUL-terminated, rest zero
  kind 2:  f32 base[16]                        world matrix (row-vector, translation = row 3)
           u32 n; n × { f32 pos[3], u32 tick }
           u32 n; n × { f32 rot[4], u32 tick } v0x1003: quaternion x,y,z,w
                                                v0x12, 0x1001: axis x,y,z + angle (rad)
  kind 3:  u32 n; n × { f32 tm[16], u32 tick }
  kind 1:  u32 frames, u32 vertices; u32 ticks[frames]; f32 pos[frames][vertices][3]
  if version != 0x12:
           u32 n; n × { f32 alpha, u32 tick }  visibility, 0..1
}
```

- **Time** (observed): ticks are 3ds Max ticks, `160` per frame, 30 fps
  (4800 ticks/s). Every key track (bone, transform, vertex) has strictly 160-tick
  steps (bone files: 1 416 207 steps; transform: 464 228; vertex: 123 tracks) and
  every transform track ends at `max_frame * 160`; so do bone tracks except in 32
  files whose keys end earlier. Looping clips end on their first pose
  (`man_knife_run`, `man_knife_idle`, `man_stun`: first/last rotation difference
  ≤ 0.001), so the period is `max_frame / 30` s.
- **Bone keys** (observed): `base` is the node's world matrix at frame 0 of *this*
  clip (not the ELU bind pose); `pos`/`rot` are parent-relative. On
  `man_knife_idle`, `base * inverse(parent base)` has the same 3×3 as the D3DX
  row-vector matrix of the frame-0 quaternion (5 bones, error < 1e-3), so a
  quaternion `q` is the usual `R(q)` on column vectors. Axis-angle (0x1001/0x12)
  matches the same local rotation to 1e-7 with `q = (axis·sin(a/2), cos(a/2))` and
  fails (error ~1.5) with the angle negated. Root bones (`Bip01`, `Bip01 Footsteps`)
  carry position tracks, spine/limbs rotation tracks; thigh bones both.
- **Unkeyed bone channels** (observed, `man_knife_idle` vs `man-set-000.elu`, bones
  present in both): unkeyed translations equal the ELU bind pose; unkeyed rotations
  equal it too except `Bip01 Neck` (3×3 difference 0.14), `Bip01 Footsteps` (0.34)
  and `Bip01 R Hand` (0.42). So the fallback for an unkeyed channel is the clip's own
  `inverse(parent base) * base`, not the ELU bind pose (`anim.rs` does this; the
  rendered poses were checked this way only).
- **Transform keys** (kind 3, observed on maps/sfx/interface): one full matrix per
  frame, e.g. sky clouds sliding. Frame 0 equals the ELU node's `world` matrix for
  3 402 of 3 441 nodes that have an ELU twin (the rest are duplicate node names,
  e.g. `Particle View 01` in `sfx/ef_dust_1m`), including nodes with a parent, so
  the matrix is a **world** matrix and the local transform is
  `inverse(parent world) * tm`.
- **Vertex keys** (kind 1, 11 files: `sfx/ef_sworddam04`, `ef_rebirth`,
  `ef_levelup`, `ef_blitz_legularhonor_gain`, `model/worlditem/ef_prop`, …):
  absolute vertex positions per keyed frame, one per ELU `positions` entry (counts
  match on all 117 tracks). Frame 0 equals `pos * world` (model space) for 102 of
  117 tracks, `pos` (local) for the 8 non-identity-world tracks of
  `ef_prop`, and neither for 7 (morph targets away from the bind shape).
- **Visibility** keys carry 0..1 (677 keys = 1.0, 586 = 0.0, rest fades) in bone
  files; present in every version ≥ 0x1001 for all kinds.
- Duplicate node names occur (97 model files, e.g. `eq_wd_katana`); playback
  drives every entity with that name.
- Bevy mapping (`src/anim.rs`, same as `model.rs`): mirror `S = diag(1, 1, -1)`;
  positions `(x, y, -z)`, matrices `S·Mᵀ·S`, quaternion `[x,y,z,w] -> [-x,-y,z,w]`
  (a reflection flips the rotation axis). Locals stay in centimetres; the model root
  carries the 0.01 scale. Rendered check: `gunz-anim` `man idle/run/stun/attack1/die`
  and `woman dance/jumpU` show upright, unbroken human poses.
- Character XML `motion_loop_type` values (`man01.xml`; `woman01.xml` within 12 of each):
  `lastframe` 410, `loop` 186, `onceidle` 164, `onceLowerbody` 59, `lonceidle` 2 (both
  `runRW`). `anim::Loop`: `loop` → `Wrap`; `lastframe` → `Hold` (stays on the last frame);
  `onceidle`, `lonceidle` → `OnceIdle` (plays once; `Animator::finished()` is the actor's cue to
  return to idle, the clip holds meanwhile; nothing separates `lonceidle` from `onceidle` in
  the data); `onceLowerbody` → `OnceLower`, played as the **upper-body layer**
  (`Animator::set_upper`) while the legs keep their locomotion clip. Why: gun
  `attackS`/`load`/`reload` (motion types 2-5, 9-11) and 2hdagger `attack1/2` carry no keys on
  the legs/calves at all (upper-body-only clips), and every non-`loop` clip of the gun types
  that fires or reloads is `onceLowerbody`; the knife/sword `uppercut`, `guard_start`,
  `guard_cancel`, `load` and the grenade/medikit/dagger `attackS` are the same type but key
  both halves (**inferred** that they are meant to leave the legs to the locomotion too).
- **Skeleton split** (observed, `man-set-000.elu` parents): `Bip01` → `Footsteps`, `Pelvis` →
  `Spine` → { `Spine1` → `Spine2` → `Neck` → `Head`, both `Clavicle` → arm → `Hand` → `eq_w*`
  attach nodes; **both `Thigh`s** }. The thighs hang off `Spine`, not the pelvis (their
  position keys are offsets from `Spine`), so the hierarchy only separates upper and lower
  body from `Bip01 Spine1` up. Every upper-body-only clip keys `Spine1`/`Spine2`/`Head`/arms;
  `anim.rs` therefore masks the `Bip01 Spine1` subtree as the upper body. Rendered check
  (`.local/shots/AnimCore/run_attackS.png`, `run_rifle_reload.png`: `gunz-anim man run --type
  2 --time 0.2 --upper attackS --upper-time 0.05`): run legs under shooting/reloading arms.
  Masking from `Bip01 Spine` instead drags the legs onto the upper clip's bind pose (seen).
- **Root motion** (observed, node `Bip01`, parent-space position, ani space Y up; `x`
  lateral, `z` forward = -z): the clip's own horizontal travel of `Bip01` is
  `anim::root_delta(ani, t0, t1)` (metres, actor frame: -Z forward, +X right). Katana
  (motion 1) over the whole clip, forward: `attack1` 0.966, `attack2` 0.758, `attack3` 1.043,
  `attack4` 1.248, `slash` 0.977, `attack1_ret` 0.545, `attack3_ret` -0.812 (back);
  `jumpwallF/B` ±0.25 (`jumpwallF` = `man_jump_wallB`, away from the wall), `jumpwallL/R` swing
  0.09 sideways and back; **zero** for `run`, `runB`, `tumbleF/B/L/R` (the dash clips) and the
  idle clips, so locomotion and tumble travel are the game's. Those clips key only `Bip01` y
  (bob) and `Footsteps`, which stay in the pose. `Animator::root_lock` pins the visual
  `Bip01` x/z at the clip's frame-0 value (y keeps bobbing); `take_root_motion` is what the
  entity must be moved by instead.
- **Events**: none for player clips; see "`system/animationevent.xml`" below.
- **Cross-fade** (`Animator::play`): nothing in the data gives a blend time (**inferred**
  by the caller). The pose on screen is captured and smooth-stepped into the new clip.
- **Aim pitch** (`Animator::aim_pitch`, **inferred** split): applied after sampling and
  layering as a model-space rotation about the lateral axis, 50 % each on `Bip01 Spine1` and
  `Bip01 Spine2` so the legs (children of `Spine`) stay put; arms, head and weapon follow.
  Rendered check: `gunz-anim man idle --type 5 --time 0.2 --pitch 45 / -45`
  (`.local/shots/AnimCore/pitch_up.png`, `pitch_down.png`).

### `system/animationevent.xml` (`anim::AnimEvents`)

**Observed**: `<NPC id>` (36 ids: goblins 11-19, lizards 21-26, skeletons 31-39, palmpoas 41-48)
-> `<Animation name>` (170; names `melee_attack` 36, `die` 36, `special_attack1` 27, `neglect1` 21,
`run` 18, `special_attack2` 17, `die2` 12, `neglect2` 3) -> `<AddAnimEvent eventtype="sound"
filename beginframe>` (233, every one `sound`, 97 distinct stems, 105 at `beginframe` 0, max 16421).
There is **no entry for the player characters and no effect, hit or footstep event**, so the
melee hit frames (`melee.rs` `STRIKE`) and the run-cycle footsteps stay **inferred** (the hit
frame is the sword-hand tip's fastest frame, footsteps the clip's half cycles); the NPC hit
moments are `zactoraction.xml` `<MELEESHOT delay>` in ms (Npc/Quest, not this file).
- `beginframe` unit (**inferred**, fits all goblin clips): 3ds Max ticks, 4800/s (= 160 ticks per
  frame at 30 fps, like `.ani` keys). `goblin_neglect.elu.ani` is 80 frames = 12 800 ticks and
  its event sits at 2358; `goblin_neglect2` is 14 400 ticks and its event at 10 232 (2.13 s); in
  milliseconds 10 232 would be 10.2 s and as a frame number 341 s, both far past a 3 s clip.
- `anim::AnimEvents::load(&vfs)` then `.get(npc_id, clip) -> &[AnimEvent { secs, sound }]`;
  `sound` is the stem relative to `sound/` (`quest/goblin/Goblin_neglect`, `blade_swing`,
  `we_grenade_explosion`). Played by the caller as `ActorSound { cue: Cue::Anim(sound) }`
  (Music resolves the stems). The parser rejects any other `eventtype`.

## Models (`*.elu`; `src/elu.rs`, `src/model.rs`, `src/character.rs`)

**Observed**: `elu::load` parses all 1 323 retail `.elu` exactly (every byte consumed):
v0x5007 1 106, v0x0 83, v0x5004 53, v0x5005 47, v0x5006 28, v0x11 6 (model 772,
maps 181, interface 173, sfx 142, quest 55). Little-endian, matrices are D3D
row-vector (`p' = p * M`, translation in the last row), **left-handed, Y up,
centimetres**; characters face -Z, their left side is +X, feet at y = 0.
Triangle winding: `cross(p1-p0, p2-p0)` points along the stored normals.

```
u32 magic = 0x0107F060, u32 version
i32 material_count, i32 node_count          (v0: both -1, see below)
material[material_count], node[node_count]
```

Material (v0x5007; earlier versions differ as listed):

```
i32 id, i32 sub (-1 = parent), f32 ambient[4], diffuse[4], specular[4], f32 power,
i32 sub_count (parent only: that many sub materials precede it in the list),
char texture[N], char alpha_texture[N], i32 two_sided, i32 additive, i32 alpha_ref
```

`N` = 256 (0x5006, 0x5007), 40 (0x5004, 0x5005, 0x11). Flags: 3 ints in 0x5007,
2 (two_sided, additive) in 0x5004-0x5006, none in 0x11. `alpha_ref` is 0 or 100
in retail. Sizes 588 / 584 / 152 / 144 bytes per material (0x5007 / 0x5006 /
0x5004-5 / 0x11). Sub materials (`sub` 0..sub_count-1) come first, then their
parent; a node refers to `id`, and a face's `sub_material` picks the sub material
when `sub_count > 0` (always `< sub_count` there; faces of single materials carry
unrelated values).

Node (0x5004..0x5007):

```
char name[40], char parent[40] ("" = root)
f32 world[16]                 world (bind) transform of the node
f32 decomp[11]                scale(3), rotation axis(3)+angle, scale axis(3)+angle   (0x5004+)
f32 aux[16]                   second matrix, unknown semantics                       (0x5004+)
i32 nv, f32 pos[nv][3]        node-local: world_pos = pos * world
i32 nf, face[nf] { i32 idx[3]; f32 uv[3][3] (u,v,w); i32 sub_material; i32 group }
                              (0x11: no `group`)
f32 normals[nf][4][3]         face normal + 3 corner normals                          (0x5005+)
i32 ncolors, f32 color[ncolors][3]                                                    (0x5005+)
i32 material, i32 nskin, skin[nskin]              nskin = 0 or nv
skin (244 B) { char bone[4][40]; f32 weight[4]; i32 unused[4] (0); i32 count; f32 offset[4][3] }
```

Evidence: weights sum to 1 on all 313 409 records (1-4 influences, `count` equals the
number of non-empty names); `offset = (pos * world) * inverse(bone.world)` (max error
1.5e-3 over 1 226 checked vertices, Y-up bind pose from `man-set-000.elu`). Skinned
nodes have `world` = identity and positions already in model space. `ncolors` is
non-zero in a few flag/cloth files (e.g. town flags: 76 vec3). Node `world` of the
skeleton gives the bind pose; local = `inverse(parent.world) * world` in column form.
Nodes named `Bip01*` are the skeleton (their stored mesh is a 6-vertex octahedron).
0x5004 and 0x11 store no normals (`elu.rs` computes area-weighted smooth ones).

Version 0 (**observed**, 83 files: interface icons, `blitzkrieg` map props, `sfx`,
`model/npc/*`): after the header (`-1, -1` in the count slots) the stream is
unaligned: `u16 node_count`, nodes, `u16 material_count`, materials. Strings are
`u16 length + bytes`. Node: `name`, `parent`, 155 floats (an axis, a scale matrix, the
**world matrix at floats 19..35**, its inverse, repeated decompositions; verified:
skin offsets reproduce to 4e-5 on `knifeman.elu`), then the 0x5007 body (`nv`, positions, `nf`,
14-word faces, normals, colors, `material`, `nskin`). Skin record: `u8 count`, per
influence `i32 bone_index, f32 weight, f32 offset[3], u16-prefixed bone name`.
Material: `id, sub, ambient/diffuse/specular, power, sub_count, texture, alpha_texture`
then 8 bytes: bytes 0-1 and 6-7 look random, per file (**unknown**), bytes 2-5 an
i32 equal to 0 or 100 (taken as `alpha_ref`, **inferred**). Two-sided/additive flags are not
recoverable (the 5007 twins of `ef_dust.tga` have flags 1,0,100, the v0 tail does not
show them). `guardian.elu` is 20 bytes with zero nodes and materials.

### Characters

`model/character.xml`: `AddXml name filename` -> `heroman1` = `model/man/man01.xml`,
`herowoman1` = `model/woman/woman01.xml` (plus two low-poly `AddElu`). The XML has
`AddBaseModel filename` (`man-set-000.elu`: 52 nodes = Bip01 skeleton, weapon attach nodes,
default body), 170 (man) / 260 (woman) `AddParts filename` (`<sex>-set-NNN.elu`, all present),
and 821 `AddAnimation name filename sound gm motion_type motion_loop_type` entries
(`motion_type` = weapon motion type of `model/weapon.xml`; retail typos such as
`lonceidle` are kept verbatim).

Every set ELU holds a full copy of the skeleton plus skinned meshes named
`eq_<slot>_NNN` with slot in `head, face, chest, hands, legs, feet` (counts over the
man sets: chest 105, legs 53, head 36, hands 25, feet 22, face 1; woman similar). A
set may provide any subset (set 003: legs, hands, chest, head). The base model
provides all six as `eq_*_000`, so the default player = base model alone; dressing
replaces exactly the slots a set provides, giving one mesh per slot. Weapon attach
nodes `eq_wd_*` (katana, sword, rifle, shotgun, rl, grenade, medikit), `eq_wl_*` (pistol,
smg, dagger, blade) and `eq_wr_*` (same four) hang off `Bip01 R/L Hand` (`wl` on the left
hand, `wd`/`wr` on the right: **observed** parents) and carry embedded weapon meshes
that the viewer does not draw (**inferred** as attach reference frames). All skin
bones of every set exist in the base skeleton; the only unresolved retail textures are
the weapon-attach reference meshes' (`pistol001.bmp`, ...).

Verified renders (`.local/shots/models/`): `man.png`, `woman.png` (bind pose, textured),
`man_set6.png` (set 6 dressed), `elu_knifeman.png` (v0 NPC).

### Bevy conversion (`src/model.rs`)

ELU -> Bevy: `(x, y, z) -> (x, y, -z)` for points and normals, `M' = S M S`
(`S = diag(1,1,-1)`) for matrices, triangle indices reversed (the mirror flips
orientation; the stored winding is front-facing in a mirrored right-handed view), model
root scaled by 0.01. Skinning uses `SkinnedMesh` with joints = node entities found by
bone name and inverse bind poses `inverse(S * bone.world * S)`.

## Items and weapons (`src/item.rs`, `src/bin/gunz-weapon.rs`)

### `system/zitem.xml` (**observed**, all 1 166 items parse)

Root `<XML xmlns="http://tempuri.org/zitem.xsd">` (UTF-8 BOM), children `<ITEM .../>`, attributes only.
Item ids are unique. By `type`: equip 543, profile 400, ticket 37, customize_effect 31, range 87,
melee 54, custom 14. `name`/`desc` are `STR:<key>` references into `system/strings.xml`
(`<STR id="key">text</STR>`, English; `#none` = unused). `system/zitem_locale.xml` is a
`<XML ID="zitem_specialized">` with only a comment and **no** items, so it contributes nothing.

Weapon items (155) are those with a `weapon=` attribute: melee 54, range 87, custom 14
(`type="custom"`: medikit, potion, repairkit, frag, flashbang, smoke). `res_sex` is `a` for all.
Always present: `delay` (ms), `damage`, `magazine`, `reloadtime` (unit unknown; 152 of 155), `weight`,
`slug_output` (`TRUE/True/FALSE/False/flase`; typo kept in retail). Optional: `range` and
`angle` (melee reach/swing, 54/14), `ctrl_ability` (recoil control, 115), `maxbullet` (101),
`snd_fire/snd_reload/snd_dryfire`, `effect_id` (79), plus potion/trap extras (`itempower`,
`damagetype`, `handweapon*`, ...), not decoded.

`weapon=` values and their counts (items that resolve to a mesh): katana 17, dagger 12,
doublekatana 11, pistol 8, pistolx2 7, revolver 7, revolverx2 8, smg 8, smgx2 7, shotgun 10,
machinegun 11, rifle 9, rocket 12, frag 2, flashbang 1, smoke 1, medikit 3, potion 4, repairkit 3.

141 weapon items have `mesh_name` (key into `model/weapon.xml`, case-insensitive). The other 14
(ids 300011-300026) are legacy NPC weapons: no mesh, no name string, referenced only by
`system/npc.xml` `<ATTACK weaponitem_id=..>`.

### `model/weapon.xml` (**observed**)

`<AddWeaponElu name=.. weapon_motion_type=M weapon_type=T><AddBaseModel name=.. filename="model/weapon/<dir>/<x>.elu"/></AddWeaponElu>`.
299 elements, 298 distinct names (`pistol20x2` is listed twice identically), 258 distinct files, all
present in the VFS; all 141 item meshes resolve. 123 distinct meshes are used by items; the others
are variants not referenced by `zitem.xml`. `weapon_motion_type` is the `motion_type` key of the
character XMLs' `<AddAnimation>` (every value used, 1-13 and 15, has animations in `man01.xml`).
Legend (from the file's comment): 1 katana, 2 1h pistol, 3 2h pistol, 4 shotgun/machinegun,
5 rifle, 6 grenade-like, 7 dagger, 8 item, 9 rocket, 10 1h smg, 11 2h smg, 12 sword, 13 blade,
14 2h dagger. One mesh (`katana_spycase`) uses motion type 15, which the legend lacks.
`weapon_type` is a finer class (1-18) the original comment calls unused. For every item mesh the
motion type equals a function of the item's `weapon` kind (`WeaponKind::motion_type`, checked by
`gunz-weapon --report`): katana 1, pistol/revolver 2, pistolx2/revolverx2 3, shotgun/machinegun 4,
rifle 5, frag/flashbang/smoke 6, dagger 7, medikit/potion/repairkit 8, rocket 9, smg 10, smgx2 11,
doublekatana 13.

### Weapon ELUs and attachment (**observed** geometry, **inferred** semantics)

Weapon ELUs have an identity root node (`eq_katana_01`, `eq_pistol_01`, `eq_rifle_001`, ...) with
rigid mesh in weapon space; some have helpers `muzzle_flash` (no vertices) and
`empty_cartridge0N` (8 vertices, shell-casing mesh, not drawn here). The character base model
(`man-set-000.elu`, `woman-set-000.elu`) holds attach nodes `eq_wd_{katana,rifle,shotgun,sword,rl,grenade,medikit}`
(parent `Bip01 R Hand`), `eq_wr_{pistol,smg,dagger,blade}` (right hand) and `eq_wl_{pistol,smg,dagger,blade}`
(`Bip01 L Hand`). Their stored mesh is the weapon itself in the bind pose: `eq_wd_katana` has
exactly the vertices of `katana01.elu` (41, max difference 0.0) and `eq_wd_rifle` the 304 of
`rifle01.elu`. So a weapon is attached by parenting its ELU root to the attach node entity with identity
transform (`Model::attach`); no further offset is needed. Left-hand dummies have a mirrored frame
(determinant -1), so the same mesh is mirrored for the left hand of dual weapons.

Attach node per `weapon` kind (`WeaponKind::dummies`; verified in `.local/shots/weapons/*-on-*.png`
for every kind on `man`, katana/pistolx2/rifle also on `woman`): katana `eq_wd_katana`; dagger
`eq_wr_dagger`; doublekatana `eq_wr_blade`+`eq_wl_blade`; pistol/revolver `eq_wr_pistol`;
pistolx2/revolverx2 `eq_wr_pistol`+`eq_wl_pistol`; smg `eq_wr_smg`; smgx2 `eq_wr_smg`+`eq_wl_smg`;
shotgun/machinegun `eq_wd_shotgun`; rifle `eq_wd_rifle`; rocket `eq_wd_rl`;
frag/flashbang/smoke `eq_wd_grenade`; medikit/potion/repairkit `eq_wd_medikit`. `eq_wd_sword`
(motion 12) is unused by any item. In the bind pose the arms hang down, so rifles and pistols point
down along the forearm and katanas point forward; guns are gripped at the handle in every render.
Idle pose (`gunz-weapon ... --on man --idle`, frame 0 of the `idle` animation whose `motion_type`
equals the weapon's): the same attachment holds the rifle shouldered, the pistol aimed forward,
the katana raised, the shotgun at the waist (`.local/shots/weapons/*-idle-man.png`).

## Effects (`src/effect.rs`, `src/bin/gunz-fx.rs`)

Labels: **observed** = checked against retail data, **inferred** = consistent with the data,
not confirmed (the executable is packed).

### Where effects are defined

| source | content |
|---|---|
| `sfx/effect_list.xml` | the registry: 137 `<AddEffectElu name>` (136 distinct; `ef_sworddam_ice` twice). Each has one `<AddBaseModel name filename>` (an `sfx/*.elu`), 128 have `<AddAnimation name="play" filename motion_type="0" motion_loop_type>` (41 `loop`, 87 `lastframe` = play once, hold the last frame), 12 carry `name_sort="1"` (documented in the file's comment as name sorting inside the model; no visible effect here), 1 has `<AddParticle name="fire" dummy_name="dummy"/>` (`ef_methor`: a particle emitter of type `fire` at ELU node `dummy`). All referenced files exist (observed). Comment in the file: models without animation are called with the animation name `play`; lit_model defaults to false for effects (no entry uses it). 7 `.elu` in `sfx/` are unreferenced (`ef_damage07/08`, `ef_lighteningball_wall`, `ef_rocket_smoke`, `ef_scrider_run`, `gz_ef_dash_heart_red`, `gz_ef_dash_heart_red2`). |
| `sfx/` | 437 files: 142 `.elu`, 134 `.elu.ani`, 98 `.bmp`, 44 `.tga`, 18 `.dds`, `effect_list.xml`. ELU versions: 0x5007 92, 0x5005 25, 0x5004 10, 0x0 9, 0x11 6. Animation kinds: transform 125, vertex 5 (`ef_death`, `ef_rebirth`, `ef_sworddam04`, `ef_levelup` = the flapping wings, `ef_blitz_legularhonor_gain`), bone 4. 57 of the 160 textures are named by no sfx ELU (blood, blood-mark, bullet-mark, muzzle_smoke, smoke, `gz_shadow`, `lens_flare_*`, `exp_num_yellow`, `ef_gz_rocket*`, ...): they belong to code-driven sprite/decal/particle systems that have no data definition (**inferred**). |
| `system/zactoraction.xml` | quest NPC action scripts: 183 `<ACTION name animation movinganimation>` with `<EFFECT delay mesh posparts posmod dirmod scale>` (345; `mesh` is an `effect_list` name, 10 distinct: `ef_gunsmoke` 109, `flame_rifle` 108 = the muzzle flash, `ef_dust_{1,2,5,10}m` 20/66/21/9, `ef_wave120` 5, `ef_guide_10m` 5, `grenade_effect`, `ef_guide120`; `posparts` is empty, `lhand` 89 or `rhand` 16), `<SOUND delay sound>` 250, `<RANGESHOT delay damage pierce sound mesh speed collradius posparts dirtarget ..>` 225 (`mesh` `ef_bullet` 220, `nullmesh` 5) and `<SUMMON>` 126. Every `mesh` resolves (case-insensitively). |
| `system/zskill.xml` | 49 `<SKILL>`; `traileffect`, `castingeffect` and `castingpreeffect` (57 values, 28 distinct attribute/name pairs) all resolve to `effect_list` names (`ef_slugs`, `ef_lighteningball`, `ef_stunfist_dam`, ...). Five skills have `castingeffectSp="BlizzardEffect"`, which is not in `effect_list` (no definition found). The `*Type`/`*scale`/`*AddPos`/`effect_startpos_type` attributes parameterise placement; their meaning is not verified. |
| `system/zitem.xml` | 79 weapon items have `effect_id` (3-9, 102, 103) that points at `system/zeffect.xml`. 31 `type="customize_effect"` items (slot `effect_dash` 21, ids 51000xx; `effect_melee` 10, ids 50000xx) carry no mesh/effect attribute: which `ef_dash*`/`ef_sword*` variant they enable is not data-defined (**unknown**). |
| `system/zeffect.xml` | 9 `<EFFECT id name knockback>` (ids 3-11: Pistol 30, SMG 20, Shotgun 400, Rifle 50, Machine Gun 150, Revolver 100, ...). Despite the name it is only a knockback table; `effect_id` 102 and 103 (2 items each) have no entry (**observed**). |
| `system/lenzflare.xml` | lens flare: 6 textures (`sfx/lens_flare00.bmp`, `lens_flare_ring00..03.bmp`, `lens_flare_grow00.bmp`) and 10 `<ELEMENT TYPE=0..9 WIDTH HEIGHT COLOR TEXTURE_INDEX>` (sizes 12-160, colour `#00FFFFFF` for all). Drawn by code relative to the sun/light (**inferred**); not implemented. |
| `system/animationevent.xml` | 36 `<NPC id>` -> 170 `<Animation name>` -> 233 `<AddAnimEvent eventtype="sound" filename beginframe>`; every event is a sound. `beginframe` is 0..16421; the unit is unverified (3ds Max ticks, 4800/s, fit better than milliseconds or frames, **inferred**). Not an effect source. |
| `system/worlditem.xml` | pickup items with `model/worlditem/ef_<colour>.elu` + looping `.elu.ani` (same `AddBaseModel`/`AddAnimation` syntax). |
| `maps/*/*.rs.xml` `OBJECTLIST` | 216 `<OBJECT name="Map_obj_....elu">` in the 86 map XMLs; all resolve to an `.elu` next to the map (some use `../`). Kinds by name (`obj_txa_fire`, `obj_flag_*`, `ani_light_add_*`, `ani_fan_*`, `obj_water_*`, `obj_sky_*`, ...). Props have no transform in the XML: their vertices are already in map space (below). `flag.xml` beside a map lists `<FLAG NAME DIRECTION POWER><WINDTYPE TYPE DELAY>` for cloth props (`obj_flag_*`). |

### `txa <frames> <ms> <file>` animated textures (**observed**)

A texture reference of the form `txa 30 2000 fire_a00.dds` is a flip-book. The literal text is
also a real file name: frame 0 is the file `txa 30 2000 fire_a00.dds` itself, frame `i` is
`fire_a<i as 2 digits>.dds` in the same directory (the trailing digit group of the stem keeps its
width: `GZ_League_CountBar_00000.png` -> `..._00012.png`). The set plays `frames` images evenly
over `ms` milliseconds and loops (the period split is **inferred**; the animation is verified only
in that frames cycle). Occurrences in retail data:

| name | where |
|---|---|
| `txa 30 2000 fire_a00.dds` / `.bmp` | 43 / 21 `.elu` materials (maps and quest maps; the same string is the DIFFUSEMAP/material name in 26 `.rs.xml`) |
| `txa 6 200 lightning_ani00.bmp` | 2 `sfx` ELUs (frames `lightning_ani01..05.bmp`) |
| `txa 91 10000 GZ_League_CountBar_00000.png` | `interface/default/combat league_popup_timebar.elu` (`txa 91 6000 ...` appears only in `system/filelist.xml`) |

All 67 ELU material sets resolve every frame (2 023 frames, 0 missing, checked with
`TexAnim::frame_name` and `view::texture_path`). No map **polygon** uses a `txa` material (0 of
the polygons of the 85 loadable maps): map fire is always an `OBJECTLIST` prop whose ELU material
carries the `txa` name (Mansion fireplaces, Castle/Dungeon/Catacomb wall torches).

### Map props (**observed** placement)

Prop ELUs are authored Y-up with vertices in world coordinates; map space is
`(x, y, z)_map = (-x, z, y)_elu`. Check: Mansion `obj_txa_fire_a_01` has centre
`(-4790, 670, 378)`, the RS lights `Omni_C_fire` sit at `(4788, 331..336, 663..667)`;
`obj_txa_fire_a_02` `(5607, 665, -778)` vs `Omni_D_fire` `(-5605, -768, 663)`. `model::spawn_elu`
maps ELU `(x,y,z)` to Bevy `(x,y,-z)`; a half turn about Y then gives Bevy
`(-x,y,z) = to_bevy(map)`. Shots: `.local/shots/fx/fire_t*.png` (Mansion fireplace at 0 / 0.7 /
1.3 s: the flame changes shape), `maps_fire.png` (Castle, Dungeon, Catacomb torches at 0 and 0.9 s).

**Which props exist** (`gunz` and `gunz-play` spawn every `OBJECTLIST` entry through `level::LevelPlugin` →
`props::spawn_props`): sky 14 of the 30 maps (Battle Arena, Blitzkrieg, Citadel, Classic Town, Factory, Garden
(2 objects), Halloween Town, High Haven, Island, Lost Shrine, Port, Ruin, Snow Town, Town), flags/curtains/banners
(Blitzkrieg 12, Town and Classic Town 7 each, Mansion 4, Garden 2, Island 1), `ani_*_light_add_*` light shafts
(Mansion, Prison, Shower Room, Factory, Dungeon), water/sea (`obj_water_*`, `obj_sea_*`, `obj_seacolor*`: Island,
Port, Garden, Citadel, Prison), fans/cranes (`ani_fan_*`, `ani_crane_*`: Station, Prison), `obj_algn*`, fires.
The other 16 maps (Castle, Catacomb, Dungeon, Dungeon New, Hall, Jail, Mansion, Prison, Prison II, Shower Room,
Skirmish Hall, Stairway, Station, Test A, Test B, Weaponshop) have no sky object.
Material modes come from the ELU material (`additive` → `AlphaMode::Add`, `alpha_ref` → mask, `two_sided`,
`.tga`/alpha-texture materials → blend), exactly as for sfx ELUs; `.elu.ani` transform tracks drive everything else.

**Sky** (**observed**: defined only by `OBJECTLIST`; no system XML or map-XML field mentions it): an ELU named
`<Map>_obj_sky_<daylight|night|sunset|box>`, `obj_ef_sky` (Island) or `obj_ef_daylight` (Port), sometimes with
a child cloud quad (`sky_ani_*` node, or a separate `*_sky_ani_cloud.elu` in Garden) whose transform track slides it
(Town: 10 451 cm over 2 000 frames = 66.7 s). Domes are authored at the world origin, 50–215 m from it
(Town ±211 m, Port ±147 m, Garden ±50 m) and enclose the playable map horizontally in all 14 maps (Island: map
x ±208 m, z −215…74 m inside the ±215 m dome). They are therefore spawned like any prop and drawn by depth, not
tracked to the camera (**inferred**: whether retail re-centres the sky on the camera cannot be told from the data).
Shots: `.local/shots/world/town0.png`, `island_b.png`, `all_montage.png` (Halloween Town moon, Factory sunset,
Blitzkrieg box, Snow Town night).

**Flags and curtains** (**observed** data, **inferred** motion): `flag.xml` (`FLAG NAME DIRECTION POWER` +
`WINDTYPE TYPE=1 DELAY`) lists 33 cloth props with wind data (Town/Classic Town `DIRECTION 90 POWER 5`, DELAY
500–1500; Mansion/Garden `0, 2`, 10 000; Island `90, 7`, 1 500; Blitzkrieg `0, 10`, 100 or 10 000) and 4 without
attributes (`test_a`, left still). Their `.elu.ani` is a transform track whose 181/2 001 keys are all identical
(maximum matrix difference 0.00 on every Town and Mansion cloth), so retail moves the cloth in code. The ELUs are
hanging sheets (Town: laundry on lines between buildings, Mansion: wall banners on rods, Blitzkrieg: team banners,
some scaled ×2.5–3.3 and rotated). `props::Cloth` models them as pinned at the top edge, vertices moving along the
thinnest horizontal axis by a travelling wave with amplitude `0.015·POWER·height` at the hem, period `DELAY`
(at least 0.5 s) and a per-prop phase; `DIRECTION` (degrees from map +X towards +Y, chosen because it makes the
Town sheets blow along their lines) sets the travel direction. The meshes are rebuilt each frame from the
bind-pose positions. Shots: `.local/shots/world/town_flag_t0.png` / `town_flag_t06.png` (folds differ),
`mansion_curtain_t0.png` / `_t5.png`, `play/town_flag_*.png` (`gunz-play`).

### Rendering sfx ELUs (`gunz-fx`)

- **Materials** (**observed**): 180 of the 183 `.bmp` materials of ELU version >= 0x5004 are additive
  (exceptions include `empty_cartridge01` and `ef_methor`); 95 of the 180 are two-sided. `.tga` materials
  with an alpha texture are alpha-blended or alpha-tested (`alpha_ref`). Version-0 ELUs (9 in
  `sfx`: `ef_gunsmoke`, `ef_dust_1m`, 7 `ef_blitz_*`) store no flags; `.bmp` materials there are
  drawn additive + two-sided and `.tga` ones blended (**inferred**; `ef_blitz_barricadebuff`
  (three shields on a ring), `ef_blitz_radarbuff` and `ef_blitz_honoritem` look right in
  `grid3.png`, `ef_gunsmoke`'s large blob (a ball textured with `ef_haste ampulla.bmp`) does not).
- **Fades** (**observed**): 131 of the 134 sfx animations carry visibility keys that drop below
  1; `Animator` reports them as `NodeAlpha`, `effect::fade_nodes` multiplies the node's meshes
  (additive: colour, others: opacity) by it. Without them every effect would stay at full strength.
- **`algn0_*` / `algn1_*` nodes** (`ef_flame_rifle`, `ef_heal`, map `obj_algn*`): crossing planes,
  `algn0` perpendicular to the effect axis (`flame_rifle`: z = const discs) and `algn1` containing
  it (x = 0 plane along the barrel). Rendered as authored, they give a star flash from the front
  and a thin spike from the side (`.local/shots/fx/grid2.png`); whether the engine also turns
  them towards the camera is **unknown**.
- **Vertex tracks** (5 files) are played by `anim::Animator` when `elu` is set (`ef_levelup`
  wings flap and fade, `.local/shots/fx/strip_levelup.png`).
- Camera/bounds come from the bind pose only, so effects that grow a lot need `--zoom`.

Verification: `fx-probe --sweep` (throwaway, not kept) loaded all 137 `effect_list` entries (137 `.elu`,
128 `.ani`, 0 failures) and the `txa` set above; `gunz-fx GAME_DIR <all 137 names> --shot` spawned
every effect together without warnings. Viewed shots: `.local/shots/fx/` - muzzle flash
(`fr_all.png`), grenade explosions (`strip_ef_exgrenade.png`, `grid1.png`), haste/heal/level-up
auras and lightning (`grid1.png`).

## HUD and sound (`src/hud.rs`)

### Interface textures (**observed**; which one the retail HUD uses is **inferred**)

`interface/default/*.png` are real PNGs (RGBA). Used: `crosshair02.png` (32x32 white cross;
`crosshair02_pick.png` is the red variant, `crosshair01/03/04/05` are other styles),
`hit_marker.png` (75x71, four diagonal ticks), `kill_marker.png` (29x29 red X),
`ingame_hpbar.png` (366x26 translucent gauge frame), `ingame_timebackground.png` (460x40 clock
strip), `ingame_reload.png` / `ingame_empty.png` (112x34 RELOAD / EMPTY badges),
`scoreboard_background_solo.png` (1060x760 plain dark gradient, alpha 134..255; used stretched).
`ingame_00.png` is an atlas (digits, "HEAD SHOT"/combo words) and `combat/hp.tga` the 2010-era HP
frame; neither is used. `combat/redbar.png` / `bluebar.png` (156x8 flat fills) are not the retail
bars any more: they read as dark brown/blue at low values.

### HP/AP bars (**observed** layout, **inferred** colour rule)

`interface/default/combatinterface.xml` (`HPAPFrame`): `CombatHPBG` = `Ingame_HPBar.png` at (87,10)
366x26; `CombatHPProgressBar` (`GRADIENTLINEARPROGRESSBAR`) inset 3 px at (90,13) 360x20 with four
`FILLCOLOR` entries, each a begin/end RGB gradient: 0 (212,212,212)->(230,230,230), 1 (232,190,58)->
(255,235,60), 2 (232,128,58)->(255,179,60), 3 (207,60,56)->(216,81,29); `EMPTYCOLOR` alpha 0.
Armour is three bars `CombatAPProgressBar1..3` at x 90/211/332, 118x20 each, blue gradients
(23,87,125)->(29,95,139), (30,97,141)->(39,109,159), (40,111,162)->(47,119,175). The data does not say
which HP colour index applies when; `gunz-play` uses index 0..3 for >=75% / >=50% / >=25% / below
(**inferred**) and fills the armour segments one after another (each a third of `max_ap`). The
fill is drawn left to right with a Bevy `BackgroundGradient`; the numbers sit next to the frame
(the retail label is white outlined text, unreadable on the white gradient without an outline).
Checked with `gunz-play ... --hp N --ap N --shot`: 100/100 (white + three full blue segments),
60/40, 30/0 (orange third, empty armour frame), 10/100 (red sliver) all show the filled share.

### Main menu and match flow (`src/menu.rs`, `src/session.rs`)

Retail art used: `bg_play.png` (1920x1080 lobby backdrop, drawn on a quad behind the 3D preview),
`gunz_logo_hq.png` (231x96), `defaultbutton_up/over.png` (140x35 dark metal buttons, stretched).
The `banner_<map>.tga` strips (360x32, a map picture with its name) exist for only 21 of the 30
maps, so the map list is plain buttons. No font file exists in the archives (`*.ttf` absent; the
retail UI uses system fonts such as the `FONTb20b` of `combatinterface.xml`), so all text uses
Bevy's built-in font. Maps are the 30 directories under `maps/` holding an `.RS` file; weapon
candidates are zitem weapons that have a name and a model (melee -> slot 1, range -> slots 2/3,
custom -> slot 4; `character` part sets: 170 for the man, 259 for the woman).

`gunz-play` without MAP opens the menu; Start re-executes the binary with the chosen flags
(`--map` preselects in the menu, `--menu-page match|player --shot` renders a menu screen headlessly);
the pause menu / match-end buttons exit with code 3 (back to the menu) or 4 (same match again) and
the launcher re-executes accordingly. Rules: see "Game modes"; the result is VICTORY / DEFEAT / DRAW
for the player and the HUD scoreboard stays up with the buttons.

### Game modes (`src/menu.rs` `Mode`, `src/session.rs`, `src/modes.rs`)

**Every retail game type** (**observed**: `GAMETYPE id` blocks of `system/gametypecfg.xml`, the
`GAMETYPE_* = id` legend and channel lists of `system/channelrule.xml`, `GAME_MODE_*` of `strings.xml`;
ids 15, 16, 18-21 appear nowhere). Status after this round, `--mode` is the CLI/menu name:

| id | retail name (`strings.xml`) | `--mode` | status |
|---|---|---|---|
| 0 | Death match solo | `dm` | done |
| 1 | Elimination (`DEATHMATCH_TEAM`) | `elimination` | done (team rounds) |
| 2 / 3 | Gladiator solo / team | `gladiator` / `team-gladiator` | done |
| 4 | Assassinate | `assassinate` | done |
| 5 | Training | `training` | done |
| 6 | Survival | `quest` | the Quest slice (`quest.rs`), not this file |
| 7 | Quest | `quest` | the Quest slice |
| 8 | Berserker (name only: `GAME_MODE_BERSERKER`) | `berserker` | done (rules **inferred**) |
| 9 | Death match team (`DEATHMATCH_TEAM2`) | `tdm` | done (respawn, team kills) |
| 10 | Duel match | `duel` | done |
| 11 | Duel tournament (`dueltournament` channel only) | `tournament` | done (knockout bracket, **inferred**) |
| 12 | Challenge quest | `quest` | the Quest slice |
| 13 | Blitzkrieg (`GAME_MODE_BLITZKRIEG`, `system/blitzkrieg.xml`, map `blitzkrieg`) | `blitzkrieg` | done (`blitz.rs`, `blitz/ui.rs`: six classes, class screen, medal / XP / bounty reward, minimap, announcer; class weapons **inferred**, medals have no currency, see below) |
| 14 | Spy (`GAME_MODE_SPY`, `spymode.xml`, `spymaplist.xml`) | `spy` | done: spy case, frost bullets, stun grenades and mines (stats **inferred**, see below) |
| 17 | Gunman (`GAME_MODE_RANDOM_WEAPON`) | `gunman` | done (weapon pool **inferred**) |
| 22 | clan scrim (`GAMETYPE_CLAN_SCRIM`, "Clan War") | `clanwar` | done offline, see "Clans": 4 against 4 (`MAXPLAYERS` 8), `ROUNDS` 3, no time limit = elimination rounds between the player's clan and a generated rival clan |
| - | "matching-only" deathmatch / team deathmatch (`GAME_MODE_MATCHING_*`), `league.xml` | - | not offline-meaningful: ranked matchmaking (`league*.xml`: Elo `elo_define`, `leaguekfactorsetting.xml` K = 50 / 30 / 20 by games played, 25 `leaguetier.xml` tiers of 100 points, one league "Classic Elimination": `deathmatch_team`, 5 rounds, 8 players, `team_kill` 0, 30 min, rating gap 300, `leaguemodule.xml` modifiers revolver damage +10 %, revolver ammo +50 %, AP +10 %); played offline it is `elimination --kill-limit 5`, the ladder and modifiers are not modelled |

Not game modes: `mvptable.xml` (17 post-match MVP awards: damage, multi kill, melee dash, jump count, ...)
and `sacrificetable.xml` (quest-item drops by quest level) belong to the profile / Quest slices.
Retail ids 8 and 14 have their `gametypecfg.xml` block commented out and 12 / 14 are commented out of every
channel, so retail had them disabled when the data was frozen; they are implemented because the data
(`spymode.xml`, `spymaplist.xml`, messages 2200-2216, the `champion` channel's id 8) is complete enough.

**Limits** (**observed**): each block of `gametypecfg.xml` lists `ROUNDS`, `LIMITTIME` (attribute `sec`, but
the labels read "10분": **minutes**; `-1` = unlimited) and `MAXPLAYERS` with one `default="true"` each; the
menu steppers use exactly these lists and defaults per mode (`Mode::limits`): id 0 50 kills / 30 min,
id 1 (Elimination) 30 rounds / 10 min, id 9 (Team DM) 70 kills / 40 min, id 2 50 / 30, id 3 and 4 30 / 10,
id 10 20 wins / 3 min per round, id 8 50 / 30, id 17 50 / 20, Spy 3 rounds. Before this round the Team and
Elimination limit lists were swapped (id 9 on the round mode); `strings.xml` names id 1 "Elimination" and id 9
"Death match team", `league.xml` runs `deathmatch_team` (= id 1) as rounds, so the lists now follow that.
Only the training default is ours (unlimited). `maps/<map>/<map>.rs.xml` has `spawn_solo_NNN` (Mansion 32),
`spawn_team1_NNN` and `spawn_team2_NNN` (16 each, two clusters at opposite ends) and
`spawn_item_{solo|team}_*`; `spawn.xml` lists items per `GAMETYPE id="solo"|"team"`. `system/blitzkrieg.xml`
has `RESPAWN baseTime="8" invincibleTime="5"` (Blitzkrieg only).

**Modes** (all single-player, bots fill the seats): *Deathmatch* (free for all), *Team DM* (player Red,
bots fill the smaller side, team kills count), *Gladiator* / *Team Gladiator* (the same with
melee-only loadouts: `Loadout.slots` is cut to the melee slot), *Elimination* (team rounds),
*Assassinate* (team rounds, one random VIP per team per round, tagged "[VIP]" in its name; a team is
out when its VIP dies), *Duel* (one-on-one rounds) and *Training* (no bots; four inert dummies 4-12 m
in front of the player, they respawn where they stood); *Berserker*, *Tournament*, *Gunman* and *Spy* below.

**Inferred** (not in the data): respawn delay 5 s (`--respawn`), spawn protection 3 s (`--protect`,
blinks, all damage ignored), round limit 180 s (`--round-time`, the duel's default `LIMITTIME`), ready
countdown 3 s (`--ready`), round-win screen 4 s, which side is `team1`/`team2`, no pickups in duel and
training. Which team's `spawn_team*` list a side uses is ours; a map without them falls back to all
`spawn*` dummies.

**Rounds** (Elimination, Assassinate, Duel, Tournament, Spy): everybody respawns at round start at their
side's spawns, protected through the countdown; `Dead.respawn` of a dead actor is pinned at
`session::HOLD` (1e6 s) until the next round; a round ends when a side has nobody left (Assassinate: its
VIP is dead; duel: a fighter is dead), or at the round limit when the side with more actors alive (duel:
more health + armour) wins, a tie is a draw. The match ends when a side has `--kill-limit` round wins
(duel: a fighter has that many wins). While the player is dead the camera follows a living teammate
(`game::Spectate`; Space or click cycles) and the HUD says so. Duel: the queue starts with the player,
the first two fight (Red / Blue teams, so bots fight each other), the winner stays at the front, the
loser goes to the back, a draw sends both back; waiting actors are dead and hidden. The first round
of a headless run with `--at` or `--bots-ahead` leaves living actors where they are. The round overlay is
hidden once the end screen shows.

Spectating, spawn points and protection are carried by the shared components `game::Spectate`,
`game::SpawnAt`, `game::Protected`, `game::Vip` and the `game::NewRound` message (item spawners
refill). `--die-at S` kills the player at match time S (headless checks).

**Berserker** (`--mode berserker`; id 8). **Observed**: only the limits above, the name, the id in the
`champion` channel and message 9910 "Deathmatch + Berserker"; every rule is **inferred**. One actor is the
berserker: its own team (Red) with 300 HP / 150 AP (three times the normal 100 / 50), name tagged
"[BERSERKER]"; everybody else is Blue, so the others never hurt each other and everybody (bots included)
hunts the berserker. Whoever kills it becomes the berserker (full 300 / 150, the old one reverts to Blue and
100 / 50 at its respawn); a berserker's death by its own hand changes nothing. The first berserker is the
lowest-id bot (the player when there are no bots). Scoring is combat's own kill count, which is exactly the
berserker's kills plus kills of the berserker. Respawns as in deathmatch. Check:
`gunz-play GAME Mansion --mode berserker --bots 7 --skill 1 --loadout 2010000,2050000,2100001 --time 50
--script wait:50 --shot OUT.png` logs `berserker: Bot 3 killed the berserker and becomes it` 7 times in 50 s.

**Duel tournament** (`--mode tournament`; id 11). **Observed**: the `dueltournament` channel (maps Hall,
Catacomb, Jail, Shower Room: `bOnlyDuelMap="true"` in `map.xml`, SkirmishHall id 27 too), message 2034 "$1 has
won by decision for dealing more damage" and the (commented-out) tip "on time-out the player who dealt more
damage wins", message 2033 "tournament points". **Inferred**: a knockout bracket over the player and the
bots, built on the duel machinery: the first two of the queue fight (Red / Blue), the winner goes to the back,
the loser is out (benched dead and hidden); a drawn bout (time-out, equal damage) is fought again; the stage
shows as FINAL / SEMIFINAL / QUARTERFINAL / ROUND OF n from the contestants left; a bout's time limit is the
duel's (`--time-limit`, 1-5 min, default 3); on time-out the fighter who dealt more damage wins (raw weapon
damage of hits not absorbed by spawn protection, **approximate**). The player stays a spectator when out;
the match ends when one contestant is left (`Round::verdict`, headline VICTORY if it is the player, else
DEFEAT). No kill limit. Check: `gunz-play GAME Hall --mode tournament --bots 3 --skill 0.7 --ready 1
--time-limit 20 --script wait:150 --time 150` logs round 1 `ROUND LOST | You are out of the tournament`, round 2
`BOT 2 WINS | Bot 2 advances (knockout)`, round 3 `Bot 3 wins the tournament`.

**Gunman** (`--mode gunman`; id 17). **Observed**: `ROUNDS 50`, `LIMITTIME 20`, `MAXPLAYERS 8/12`,
`randomweaponmaplist.xml` (Mansion, Station, Town, SkirmishHall; not enforced by the menu), achievements
"Play Gunman mode". **Inferred**: deathmatch where every life (spawn and respawn) brings one random melee
weapon and one random gun, as slots 0 and 1, gun selected; the pool is the first named item with a model and
damage > 1 of each kind (3 melee: katana, dagger, double katana; 10 guns: pistol, dual pistols, revolver,
dual revolvers, SMG, dual SMGs, shotgun, machine gun, rifle, rocket launcher). Mechanism (shared, also Spy):
`game::Arsenal` makes `ActorSpawner` spawn every actor with the whole pool's weapon models, `game::Equip`
(handled in `actor.rs`) then re-picks which of them are the loadout slots and how many rounds/grenades each
has. Check: `gunz-play GAME Mansion --mode gunman --bots 3 --time 14 --die-at 4 --respawn 2 --script wait:14`
logs `gunman: Player gets Iron Kodachi AS + Boiler Cannon GL` at spawn and `... Rusty Dagger AS + Nico MG-K8 MK1`
after the respawn.

**Spy** (`--mode spy`; id 14, needs at least `BASE minPlayer` = 4 actors, else free play). **Observed**
(`spymode.xml`): `SPY_TABLE` per player count 4..12 gives the number of spies (1 for 4-6 players, 2 for 7-9,
3 for 10-12), their health and armour `HPAP` (50 / 100 / 150, 50 / 80 / 110, 50 / 70 / 90), flashbangs
`LIGHT` and smoke bombs `SMOKE`, frost bullets `ICE`; `TRACER_TABLE` gives a tracker `STUN` 2 stun
grenades and `MINE` 10 mines; `SELECT_SPY` has `selectSpyTime` 10, ratings 1000 / 500 / min 500 / max 1500
and `RounFinishWaitTime` 3 (sic); `SPY_ITEM_DESC` maps zitem ids 601001-601006 (LIGHT, SMOKE, ICE, STUN,
MINE, BAG) which are **not in `zitem.xml`**; `spymaplist.xml` gives 17 maps with `minPlayers`/`maxPlayers`,
`limitTime` 50-150 s and `spyOpenTime` (always one fifth of `limitTime`); messages 2200-2216: "Spies are
unable to use conventional weapons, but they gain exclusive access to Frost Bullets, Flashbangs, and Smoke
Bombs", "Trackers are provided with Stun Grenades and Antipersonnel Mines", "Your location has been
compromised; avoid the Trackers to survive!", "The Spies' locations have been successfully triangulated; you
must hurry!", "Spy's Identity:", "You will join the game at the start of the next round". **Implemented**:
rounds; the round time is the map's `limitTime` (`spymaplist.xml` via the `map.xml` id; a map not listed keeps
180 s, `--round-time` overrides); the first spies are drawn `selectSpyTime` (10 s) into the match, later rounds
draw at their start; a spy carries the spy case (`BAG`),
frost bullets, smoke bombs and flashbangs (counts from the table, HP = AP = `HPAP`) and no conventional
weapon, a tracker the normal loadout plus `STUN` stun grenades and `MINE` mines; everybody is one team until
the spies are located `spyOpenTime` into the round (the map's row; a fifth of `limitTime` in every row, the same
ratio is kept when `--round-time` changes it; nobody can be hurt, stunned or slowed before: bots do not
find the spies early), then the spies turn Blue, get a "[SPY]" tag and both sides see the two retail banners;
the **triangulation hint** (`spy.rs` `ping`): the tracker player gets a red "name distance" marker per living
spy, pinned where it stood at the last ping and refreshed every 3 s (**inferred** period: messages 2201/2202
only say the locations were "triangulated"); a spy off screen sits on the left or right edge; a spy player
gets no marker (it only hears 2201);
the trackers win by killing every spy, the spies by surviving to the limit; the round result lists "Spy's
Identity". The match ends when the player's side has won or lost `--kill-limit` rounds (default 3). **Inferred**:
the spy draw (ratings start at `DefaultRating`; after a round the last spies drop to `SelectedRating`, the
rest rise half way to `MaximumRating`, never below `MinimumRating`; the highest ratings are the next spies,
ties by a hash), reading `selectSpyTime` as a one-off delay from the start of the match (the file comment
says spies are picked `selectSpyTime` seconds after the game starts), the one-fifth reveal time for unlisted
maps, survive-to-win, the 3 s result screen (`RounFinishWaitTime`).
Attribute status: `BASE minPlayer` used; `SELECT_SPY` `selectSpyTime`, `DefaultRating`, `SelectedRating`,
`MinimumRating`, `MaximumRating`, `RounFinishWaitTime` used; `SPY_ITEM_DESC` used (ids); `SPY_TABLE`
`TotalCount`, `SpyCount`, `HPAP`, `LIGHT`, `ICE`, `SMOKE` used, `BAG` read as always 1 (the file says so);
`TRACER_TABLE` `STUN`, `MINE` used; `spymaplist.xml` `id`, `limitTime`, `spyOpenTime` used,
`minPlayers`/`maxPlayers` only as a warning when the actor count is outside them (matchmaking, no
offline meaning), `name` ignored (`map.xml` gives it). Nothing unused. Check: `gunz-play GAME Factory --mode spy
--bots 3 --skill 1 --ready 1 --shot OUT.png --script wait:24 --time 24.5` logs `round 1: spies ["Bot 1"] of 4`
10 s after the start (selectSpyTime), `spies located` 10 s into the round and `spy ping`
every 3 s; `--bots 3 --skill 1 --round-time 10 --ready 1 --kill-limit 6 --time 120 --script wait:120` plays whole
rounds to `SPIES win the round`.

**Spy items** (`item::SPY_*` and `Items::load`, `src/spy.rs`, `src/projectile.rs`, `src/bot.rs`). The ids are
**observed** (`SPY_ITEM_DESC`: 601001 `LIGHT` and 601002 `SMOKE` are the zitem flashbang 2200001 and smoke
2200002 the mode already used; 601003 `ICE`, 601004 `STUN`, 601005 `MINE`, 601006 `BAG` are in no zitem), so
`Items::load` adds the four as clones of a stand-in zitem with the model the data gives them. The weapon
stats are **inferred** (the data has none):

- `BAG` 601006 is the **spy case**: `model/weapon.xml` has `katana_spycase` (`weapon_motion_type` 15,
  `weapon_type` 1 = katana; comment "스파이모드 전용 아이템" = spy-mode-only items) and `man01.xml` /
  `woman01.xml` define motion type 15 with its own clips (`run`, `attack1/2` and their `_ret`, `attack_Jump`,
  `uppercut` = `*_spycase_smash`, `guard_idle`, `guard_block1/2`; the rest are the knife's), plus the quick-slot
  icon `interface/default/combat/icon_spy_spycase.tga`. So the bag is a blade (`WeaponKind::SpyCase`, melee
  rules of a katana: stand-in 2010000, 30 damage, 220 cm, dummy `eq_wd_katana`); it is slot 0 of a spy.
- `ICE` 601003 **frost bullets**: the count per spy is observed (4-6). The data has no spy gun (the only
  `spy_*` models are the stun grenade and the mine), so a frost bullet is a revolver stand-in (2050000:
  30 damage, 11 m best range for bots) whose hit also slows its target to 50 % for 7 s (`spy::FROST_*`;
  the retail Slow skill 151 / 165 is the only slow in the data, tip 2203 says the effect is "devastating").
- `STUN` 601004: model `spy_stungrenade` (motion 6, a grenade), flashbang stand-in (2200001: radius, 1.5 s
  fuse). It stuns everyone that is not an ally of the thrower within the radius and in the open for 3 s
  (`spy::STUN_SECS`, the retail Stun skill 351; tip 2206 "briefly stunned"), no damage; the thrower is safe.
- `MINE` 601005: model `spy_landmine` (motion 8, `weapon_type` 12, held like a medikit), frag stand-in (2200007:
  55 damage, 4 m, magazine 1, so a reload clip passes between two). A click lays it on the floor 0.8 m ahead
  (under the layer when there is no floor there); after 1 s it goes off when a non-ally of its layer comes
  within 1.5 m (flat; `spy::MINE_*`), as that frag blast with its knock-up (the layer's own blast hurts the
  layer). Tip 2207 "The Spy can also see mines that have been installed": mines show to a spy player and to
  their layer (**inferred**: you know where you laid it), nobody else; they last one round.
- Bots follow the weapon rules they already had: a spy uses the case under 3.5 m and frost bullets as a
  revolver, throws flashbangs at enemies behind cover; a tracker throws stun grenades by the flashbang rule.
  New, simple rule: a bot with mines lays one every 6-12 s (first after 4 s) while its nearest enemy is over
  6 m away (it switches to the mine, stands still 0.6 s and clicks; gives up after 8 s). Smoke bombs are
  never thrown by bots.

Checks (headless, `--bots 1 --skill 0.2`, `RUST_LOG=gunz=info`): `--loadout 601003` and `attack` logs
`damage: Player -> Bot 1 30` then `status: Bot 1 <- slow x0.50 ... for 7.0 s`; `--loadout 601004`
`detonation: Stun of Player` and `status: Bot 1 <- ... stun true ... for 3.0 s`; `--loadout 2010000,601005
--bots-ahead 1.8 --script "2:0.4;attack:0.3;wait:12"` logs `mine: Player lays one`, `mine: goes off`,
`detonation: Mine of Player` and the damage to both; `--loadout 601006 --bots-ahead 1.6` logs `clip attack1
(motion 15)` and the hit. In a spy round of 6 (`--mode spy --bots 5 --round-time 100 --kill-limit 1`) the
bots lay mines, a tracker's stun grenade stuns the located spy, frost bullets slow trackers.

**Blitzkrieg** (`--mode blitzkrieg`, id 13; `src/blitz.rs`, the soldiers and buildings are `npc.rs` actors). Only on
the map `blitzkrieg` (`gunz-play` exits with an error for another MAP, the menu picks the map). **Observed**:

- `system/blitzkrieg.xml`, all of it read by `blitz::parse` (test `rule_book_parses`): honor start 470, +2 every
  1 s (`LEAVE_AUTO_INC_HONOR`: 3 / 4 / 8 with 3 / 2 / 1 players left), first kill +50,
  `RESPAWN baseTime 8 invincibleTime 5`, `FINISH_DELAY_TIME 8`, `ENHANCE_PLAYER apHp 75 dps 60`, `ENHANCE_NPC`
  (every 90 s, 20 times, +6 %), `BUILDING reduceDamageRatioFromPlayer 0.93` (a building takes 7 % of what players
  deal; message 2121 says "94 %"), `BARRICADE dist 800 reduceDamageRatio 0.5` (message 2124: "only half the damage")
  and `RADAR dist 600 recoveryApHpRatio 0.1 recoveryMagazineRatio 0.1 recoveryDelay 0.7` (message 2125: "HP/AP/Bullet
  will restore"), `HONOR_ITEM_LIST respawnTime 120` (`tresure1-4`), `REINFORCE_LIST` (9 / 6 / 3 barricades left: that
  side's radar goes to `summon_zealot` / `_cleric` / `_knight`; 0 left: the **enemy** radar `summon_terminator`,
  message 2127), `UPGRADE` (six attributes x 4 steps: cost 250 / 325 / 400 / 1200, DPS 40:40:40:120, shot delay
  0.25:0.25:0.25:0.75, AP/HP 65:65:65:195, fire 7:7:7:21 for 4 s, bullets 0.5:0.5:0.5:1.5, respawn -0.2 / 0.35 /
  0.5 / 0.7), `WEAPON` (per kind DPS factor and shot delay ms), `HONOR_LIST` (player kill 50 + victim's total honor
  / 50, assist 25 + total / 100 within 5 s; per actor `type`: barricade 30 / team 60, honor_item 20 / 35, knifeman
  5, throwman 10, zealot 40, cleric 50, knight 60, terminator 50 / 150), `SPAWN_LIST` (radar 1, barricade 12,
  guardian 1 per side, team 2 red / 3 blue), `ROUTE_LIST` (8 routes), `CLASS_TABLE` (9 rows) and `CLASS_BOOK`
  (gladiator, duelist, incinerator, combatofficer, assassin, terrorist), `CLASS_SELECT_TIME 30`,
  `LEAVE_AUTO_INC_HONOR` 3 / 4 / 8 (income with 3 / 2 / 1 players left), `REWARD`, `EVENT_MESSAGE viewTime 4
  delayTime 1 damagedRadarCoolDown 2 sound_Benefit/Loss`, `HELP_MESSAGE viewTime 4 dist 500 honor 300 sound`. Only
  `PENALTY` (120 / 300 / 600 s lock-out after quitting) is unused. Messages 2100-2130 (`system/messages.xml`, English)
  carry the texts: 2100 class countdown, 2113 / 2120 reward bonuses, 2115 / 2116 class limits, 2119 respawn
  protection, 2121-2128 the help sentences.
- Map `blitzkrieg.rs.xml` dummies: `spawn_blitz_radar_{red,blue}` (x = +/-72 m), `spawn_blitz_barricade_{red,blue}_0..11`
  (x 21-53 m: three rows across the lanes), `spawn_blitz_guardian_*` (x +/-82 m, on the spawn platform 8 m up),
  `spawn_blitz_honoritem_0..3` (centre of the map), `route_{top,mid1,mid2,bot}_1..8` (the lanes run from x +72 m to
  -72 m; ROUTE ids 100/210/220/300 start at the red radar, 101/211/221/301 at the blue one; 220 and 221 end on a
  `route_mid1_*` node, as the file says), `spawn_team1_101..104` (red, x +78 m) and `spawn_team2_*` (blue).
- `npc2.xml` / `aifsm.xml` / `zactoraction.xml`: `radar_*` FSMs enter `summon` (action `radar_summon_red1`: 13
  `SUMMON name range=400 angle route` events over 1.6 s, knifemen then throwmen), wait 30 s and summon again;
  `summon2` (throwmen only) is no state's target; `summon_zealot|cleric|knight|terminator` have cooltime 99 999 999 and
  no transition leads to them: the mode forces them. The soldiers' `recon` runs `runWaypointsAlongRoute,
  findTargetInDist:1200..1300` (terminator 600): a route walker that attacks what it can see. The barricade FSM changes
  its animation at `groggy` 333 / 666 (`destroy33`, `destroy66`). `guardian.elu` is a 20-byte file with no node: the
  guardian is an invisible 60 000 HP actor with a 1 cm collision capsule whose only skill is `guardian_shockwave` (100
  damage, 10 m, all around, thrust) at anything within 7.5 m and 6 m of height: a spawn guard, **not** a target in
  practice.

**Implemented** (constants from the file unless marked **inferred**):

- `game::SpawnNpc` has `team` and `route`; `game::Routes` maps a route id to waypoints (floor-snapped dummy
  positions) and carries `ENHANCE_NPC`'s boost, which `npc.rs` applies to every routed spawn's health;
  `Ev::Summon` reads `route`; `game::NpcState` forces a state machine into a named state; `game::Mods`
  (damage dealt / taken, taken from players, gun delay) is applied by `combat::apply_damage` and the actor controller.
- `npc.rs`: `runWaypointsAlongRoute` marches along the route (next waypoint within 2.5 m, **inferred**; the last one is
  held) whether or not a target is known; monsters now fight *every* hostile with `Vitals`, other monsters too (a
  `Foe` has the target's hit capsule, `distTarget` and melee reach go to its surface; soldiers ignore teamless
  honor crates; buildings spark, they do not bleed); the entry state's action starts at once (the radar's first wave
  leaves at t = 0). Soldiers fight enemy soldiers, bots, players and buildings; bots (`bot.rs`, unchanged) already hunt the
  nearest non-ally, which includes enemy soldiers, crates and buildings.
- `blitz.rs`: objectives from the dummies; teams (`session::teams`: the player is Red, bots fill the smaller side,
  Blue first), respawn 8 s / 5 s protection (`--respawn` / `--protect` override), respawns at the team's
  `spawn_team*` dummies (a run without `--at` / `--bots-ahead` starts there too), no kill limit; honor: start, income,
  kills (`single`, `all` to the whole killing side, soldiers killing buildings pay the humans on their side),
  player kills (kill + total / 50, first kill of the match +50, assists within 5 s), `GUNZ_BLITZ_HP` aside; buildings
  take 7 % of player / bot damage and a player inside a friendly barricade's zone takes half; a radar heals 10 % of
  maximum health, armour and ammunition every 0.7 s within 6 m, a barricade restocks 10 % of the ammunition every 2 s
  within 8 m (zone height 6 m, **inferred**); crates come back 120 s after being destroyed (they exist from the start,
  `tresureN` at `spawn_blitz_honoritem_{N-1}`, **inferred** pairing); reinforcements and the terminator as above;
  the match ends when a radar or guardian is destroyed (8 s later the end screen, VICTORY if the player's side
  survived), or at `--time-limit` for the side with more barricades left (equal: DRAW; **inferred**).
- Upgrades (`F` opens the panel, Up / Down choose, Enter buys; bots buy by themselves in the order armour, power, rapid
  fire, magazines, fire, medics): **power** adds `(60 + steps) x factor x delay` damage to each hit of the current
  weapon (**inferred** reading of `WEAPON`; `Mods.dealt`), **rapid fire** divides the gun delay by 1 + the steps'
  sum (`Mods.shot_delay`; **inferred**: "+N % shooting speed"), **armour** raises maximum and current AP and HP, **fire
  rounds** burn the target for N per second for 4 s through `Afflict` (per second is **inferred**), **magazines**
  scale the spare ammunition (again after every respawn; the radar / barricade refill uses `max_bullet` as the
  reserve cap, **inferred**), **medics** shorten the respawn by the table value at the step reached (the table is
  read as cumulative, **inferred**).
- **Classes** (`CLASS_SELECT_TIME 30`; screen in `src/blitz/ui.rs`, test `rule_book_parses`): the match starts held
  (`game::Hold`: frozen like a pause but without the pause menu, Esc does not resume) with six cards, the countdown of
  message 2100 and the highlighted class taken when it runs out (30 s of real time); 1-6 / arrows choose, Enter / Space
  or a click confirms. Bots pick a random class with at most 3 per side (message 2116; its Korean text reads "3 or
  more", the English and Chinese "more than 3"). `--shot` runs skip the screen (no class, the default katana /
  revolver / rifle) unless `GUNZ_BLITZ_SELECT=1`; `GUNZ_BLITZ_CLASS=N` picks N without it. Effects (**observed**
  numbers, **inferred** reading): Gladiator +60 AP and HP, +60 DPS with a blade (as `ENHANCE_PLAYER dps`); Duelist
  +3 shotgun magazines and shotgun damage x2 (`enhanceShotgunDamage="1"`, read as a share like the file's other
  0..1 values and like the Terrorist's `1.0`); Incinerator 7 fire damage per second for 4 s on every hit and -20 DPS;
  Combat Officer: allies in 8 m (himself included) take 15 % less (`checkDelay` ignored, evaluated every frame);
  Assassin +15 % damage; Terrorist +100 % damage to buildings (`Mods.vs_buildings` against `Mods.building`, applied
  in `combat::apply_damage`). **Weapons are inferred** (the data gives no loadout): Gladiator katana + revolver,
  Duelist dagger + shotgun, Incinerator katana + the machine gun the data names "Incinerator" (2110008), Combat
  Officer katana + rifle, Assassin dagger + SMG, Terrorist katana + rocket; every actor spawns with all of them
  (`ModesPlugin::finish` -> `blitz::arsenal`, as Gunman) and an `Equip` picks the two. A Blitzkrieg actor therefore
  ignores the profile's loadout.
- **Honor income and `LEAVE_AUTO_INC_HONOR`**: with 3 / 2 / 1 players left on a side (alive or dead, players and
  bots) each of them earns 3 / 4 / 8 honor per second instead of 2; offline "left" means the side never had more
  (`--bots 2` gives 2 : 2 and the faster income).
- **Reward** (`REWARD`, `blitz::payout`, test `rule_book_parses`; the panel is `ui::reward_ui`): nothing is paid
  under `minTime` 420 s or `minHonor` 2000 total honor (**observed**); medals are 15 (win) / 5 (loss) plus 1 per
  minute up to 20, the MVP of a side gets +15 % (winners) / +45 % (losers) of XP, bounty and medals (**observed**;
  message 2120 "You have won additional $1% of XP/BP/Medal" is its text). **Inferred**: XP and bounty are
  `baseExp` / `baseBounty` (50 each) per full minute played, the same for both sides; a draw pays like a loss;
  the MVP is the player with the most honor earned on the side. XP, bounty and medals are paid once through
  `game::Reward` (the profile also pays its usual match result on top); the profile stores the medals (`medals=`)
  and shows them on its header. **Impossible**: a medal shop. The data has `interface/default/medalshop.xml` (UI
  frames only), `SELL_GROUP` 3 in the `gshop.xml` header comment, "medal" strings, and no price in medals anywhere
  (`gshop.xml`'s `PRICE` is bounty/cash, `eventshopitem.xml` is an event-coin shop, `zitem.xml` has no medal price),
  so nothing can be bought with medals. `minPlayCount`
  (a newcomer bonus counted in games played), the waiting medals (matchmaking) and `PENALTY` / message 2112 (a
  quit is the application closing) have no offline counterpart.
- **Minimap** (`ui::floor_plan`, `ui::minimap`, test `plan_fills_the_floor`): no retail minimap texture exists
  (`interface/` has only `map_blitzkrieg.bmp`, an 800 x 92 street banner, and an empty `blitzkrieginterface.xml`;
  messages 2111 / 2118 say a key switches the board between status, minimap and help), so the plan is drawn from the
  map's own upward-facing polygons (highest per pixel, shaded by height) at the right edge above the ammunition
  (`M` hides it), the Red base on the left; dots: the player (white), players and bots, soldiers (green allies / red
  enemies), barricades (squares) and radars (large) tinted blue for the player's side and orange for the other.
- **Announcer** (`EVENT_MESSAGE`, `HELP_MESSAGE`, `blitz::events` / `helps` / `feedback`): messages queue and show
  for `viewTime` 4 s (`delayTime` 1 s while another waits); a radar under attack (at most every
  `damagedRadarCoolDown` 2 s per side), a destroyed barricade and a reinforcement wave play `Blitzkrieg/EventBenefit`
  when they favour the player's side and `EventLoss` otherwise (**inferred** pairing; the texts are mine, message 2126
  is the enemy wave's); the help messages 2121-2128 play `Blitzkrieg/Help` once each when their situation arises
  (`honor` 300 for 2123, `dist` 500 for 2121 / 2122; the other triggers are **inferred** from the wording). Honor
  gains of the player play `ef_Blitz_{Less,Legular,More}Honor_Gain` and `{less,regular,more}gainhonor.wav` below 30 /
  below 100 / above (**inferred** thresholds); the buff effects `ef_Blitz_RadarBuff` / `BarricadeBuff` /
  `CombatOfficerBuff` replay every 1.5 s while the player stands in the radar zone, a barricade zone or an officer's
  reach, `ef_Blitz_HonorItem` plays where a crate comes back. The data has no effect for an upgrade purchase (the
  seven `ef_blitz_*` models are those), so a purchase has only the banner. `hit_rader`, `hit_barricade`, `radar_die`
  and `radar_work` were already played by `npc.rs` (`neverblasted.sound`, `sound.die`, the radar actions).
- HUD: the kill counter's line shows `HONOR n   [F] upgrades   CLASS`, the panel lists the six attributes with the
  next step's value and cost, a banner shows purchases and honor gains, the line below it the announcer's messages,
  the header shows `BARRICADES RED n : m BLUE`. Headless hooks: `GUNZ_BLITZ_BUY="SECS:N,.."` buys upgrade N (1-6) for
  the player and opens the panel, `GUNZ_BLITZ_HP=K` scales radar, barricade and guardian health,
  `GUNZ_BLITZ_SKIP=SECS` starts the match SECS seconds in (clock, enhancement of the waves and the honor income of
  those seconds), so a short run can reach the reward's 420 s / 2000 honor minimums.
- **Guardian**: `guardian.elu` has no node, so the actor has no model: it stays an invisible 60 000 HP spawn guard
  (observed: `npc: spawned guardian ... hp 60000` at x = +/-82 m, 8 m up; the guardian's death would end the match like
  a radar's, but a 1 cm capsule cannot practically be hit) and is not drawn on the minimap.

**Not modelled**: the class books (900000-900003 are in `globbyuseableitem.xml` / `gshop.xml` as the bounty coins
"Bounty Pack ... Chest" worth 10 / 100 / 1000 / 10 000, 900004 a 5 000 chest, so the class books named by `CLASS_BOOK`
are not in this build: every class is free to pick), the three classes without a book (`HUNTER`, `SLAUGHTER`,
`TRICKSTER` have `CLASS_TABLE` rows but no book and no description anywhere), class names and descriptions in the
data (none: the names are the `CLASS_BOOK` keys, the card texts are written from the table), class icons (none), the
medal currency, `PENALTY`; a soldier's `suffer*` states react to damage only as far as `npc.rs` models groggy.

**Checks** (headless, logs and shots in `.local/shots/Blitz/`; `GAME` is the Steam install directory):

- `gunz-play GAME blitzkrieg --mode blitzkrieg --bots 6 --skill 0.8 --time-limit 180 --shot match180.png --script
  wait:181 --time 181` with `GUNZ_BLITZ_BUY=20:1,45:3,70:2`: waves leave at t = 0 and every 30 s (`npc: spawned
  blitz_knifeman_red ... Red route 100`), the lanes meet at the centre around t = 10 s (`damage: blitz_knifeman_blue ->
  blitz_knifeman_red 20`, `kill:` lines between soldiers), the bots shoot crates and soldiers (`blitz: BLUE killed a
  honor_item: honor ...`), soldiers and bots chip the barricades (`BARRICADES RED 10 : 12 BLUE` at 180 s), bots and
  player buy upgrades (`blitz: Player buys Weapon power step 1`), and the time limit ends it: `blitz: time is up,
  barricades RED 10 : 12 BLUE`, DEFEAT (`match180.png`). A status line every 20 s counts barricades and soldiers.
- The same with `GUNZ_BLITZ_HP=0.08 --die-at 50` (`fast.log`): barricades fall from t = 38 s (`blitz: BLUE killed a
  barricade: honor Bot 4 +60 ...`), `RED has 9 barricades left: RED radar calls zealot`, `6 barricades left: ... calls
  cleric`, burning rounds (`status: Bot 4 <- ... dot 28 for 4.0 s`), and the player killed at 50 s respawns at 58 s.
- End of a match: `GUNZ_BLITZ_HP=0.005 gunz-play GAME blitzkrieg --mode blitzkrieg --bots 0 --hp 5000 --ap 5000 --at
  -6000,0,20 --yaw 90 --npc blitz_terminator_red --bots-ahead 7 --script wait:26 --time 26` (`radar_down.log`): the
  terminator kills `radar_blue`, `blitz: BLUE radar destroyed`, the header reads `BLUE DESTROYED - RED WINS`, and 8 s
  later the VICTORY scoreboard (`radar_down.png`).
- A natural game does not end in three minutes: the 93 % resistance of the buildings and the symmetric waves leave
  the front near the centre until players, bots and the reinforcements tip it (the retail `REWARD minTime` is 420 s).
- Classes, minimap, announcer and reward (logs and shots in `.local/shots/BlitzMore/`; `GUNZ_PROFILE` a throwaway
  file, `RUST_LOG=off,gunz::blitz=info`):
  - `GUNZ_BLITZ_SELECT=1 gunz-play GAME blitzkrieg --mode blitzkrieg --bots 5 --shot select.png --time 2`: the held
    class screen, "Please select your class in 27 second(s)" (`select.png`, the minimap behind it).
  - `GUNZ_BLITZ_CLASS=2 ... --bots 5 --time 3` (`class2.png`): `blitz: classes: Player (RED) Duelist; Bot 1 (BLUE)
    Terrorist; Bot 2 (BLUE) Combat Officer; Bot 3 (RED) Incinerator; Bot 4 (BLUE) Gladiator; Bot 5 (RED)
    Terrorist;`, the player holds the Duelist's shotgun with 6/42 (24 spare + 3 x 6), `DUELIST` on the honor line,
    the minimap with the Red base on the left.
  - Announcer (`GUNZ_BLITZ_CLASS=4 GUNZ_BLITZ_HP=0.08 ... --bots 6 --skill 0.8 --die-at 50 --time-limit 120`):
    `announce [Blitzkrieg/EventBenefit] ALLIED REINFORCEMENTS: zealot`, `[Blitzkrieg/EventLoss] YOUR BARRICADE WAS
    DESTROYED`, `[Blitzkrieg/EventBenefit] ENEMY BARRICADE DESTROYED`, `[Blitzkrieg/EventLoss] The enemy's
    reinforcements have arrived. ...`; the announcer run ends at the time limit with `blitz: reward DEFEAT: +0 XP,
    +0 bounty, +0 medals (needs 420 s of play)`.
  - Reward: `GUNZ_BLITZ_CLASS=6 GUNZ_BLITZ_SKIP=800 GUNZ_BLITZ_HP=0.005 ... --bots 0 --hp 5000 --ap 5000 --at
    -6000,0,20 --yaw 90 --npc blitz_terminator_red --bots-ahead 7 --script wait:26 --time 26` (`reward2.png`; with
    `gunz::audio=info` the log shows `announce [Blitzkrieg/EventBenefit] ENEMY RADAR UNDER ATTACK` followed by
    `sfx eventbenefit`, and `announce [Blitzkrieg/Help] ...` by `sfx help`): the radar falls, `blitz: reward VICTORY
    (MVP): +748 XP, +748 bounty, +32 medals` (13 minutes, 15 + 13 medals, MVP +15 %), the panel beside the
    scoreboard and the profile's `LEVEL UP 1 -> 4  +798 XP`. With bots (`reward.png`, `--bots 5 --time 110`) the
    player is not the MVP: +650 XP, +28 medals.

### Sounds (**observed**)

`sound/` holds 452 playable files: 437 `.wav` + 15 `.ogg` (bgm); `sound/effect/` has 4 wavs that
are IMA ADPCM (format tag 17: `we_new_desert_1`, `we_new_handgun_2`, `we_new_shotgun_1`,
`we_new_sniper_fire_2`; no zitem item names them, bevy cannot decode them), the rest PCM 8/16-bit
mono or stereo (448 of 452 decode with bevy's rodio/hound). `sound/effect/effect.xml` lists per-sound
`MINDISTANCE`/`MAXDISTANCE` (cm) and `type` (0 3D, 1 2D, 2 2D+3D, 3 2D stereo, 4/5 2D loop,
6 3D loop); weapon sounds exist twice: `we_x_fire` (mono, 3D) and `we_x_fire_2d` (stereo, type 3).

Name -> file: a zitem sound name is a file stem under `sound/effect/` (`we_rifle_fire` =
`sound/effect/we_rifle_fire.wav`).

| source | names | files |
|---|---|---|
| zitem `snd_fire` | `blade_swing`, `we_{pistol,revolver,smg,shotgunpa,rifle,machinegun,rocket,grenade}_fire`, `swing` | all resolve; `swing` (one legacy NPC dagger, item 300012) has no file and is aliased to `blade_swing` (**inferred**) |
| zitem `snd_reload` | `we_{pistol,revolver,smg,shotgunpa,rifle,machinegun,rocket}_reload` | 7/7 |
| zitem `snd_dryfire` | `357magrevolver_dryfire`, `762arifle_dryfire` | 2/2 |

Distinct zitem sound names resolving: 18 of 19 as named, 19 of 19 with the `swing` alias. All 155 weapon items resolve a fire sound
(114 carry `snd_fire`; the shop melee weapons carry none and use `blade_swing`, the sound of
the 12 legacy melee items with `snd_fire`; **inferred** default). `model/man/man01.xml` and
`woman01.xml` give 39 animations a `sound` attribute: `man_jump` (15, `jumpD`; files
`man_jump_mt_<material>`) and `fx_dash` (24, `tumble*`, file `fx_dash`). Footsteps are not tagged
in the animation XML: files `man_fs_{l,r}_mt_<material>` with materials `con drt met pnt snd snw
wat wod` (**inferred** naming: left/right foot, surface); the game plays `_mt_<material>` by looking up the polygon under the feet (see below). `system/animationevent.xml` only has `<NPC id>` entries (36 NPCs,
233 sound events, 97 distinct files, all resolve under `sound/effect/` or `sound/effect/quest/<monster>/`)
with `AddAnimEvent eventtype="sound" filename beginframe`; it has nothing for
the player characters and is not used by `gunz-play`.

### Sound playback and feedback (`src/audio.rs`, `src/hud.rs`)

- **Surface** (**inferred** from names): a map material name's suffix `_mt_con/_drt/_met/_wod/_pnt/
  _snd/_snw/_wat/_gls/_fsh` selects the `_mt_<x>` file of footsteps, jump/land, bullet hit and shell
  drop; polygons without a suffix are concrete. Only the non-concrete polygons are indexed
  (Mansion 8 792, Dungeon 1 005).
- **Ambience**: the map's `AMBIENTSOUNDLIST` (`snd_amb_*` dummies; `effect.xml` type 6 = 3D loop,
  4/5 = 2D loop), at most 8 nearest loops play. Mansion has 3, Dungeon 74.
- **BGM**: see "Music" below (no data file maps a map or mode to a track; the choice is **inferred**).
- **Visual**: bullet-hole / blood-mark decals from `sfx/*bulletmark*`, `sfx/blood-mark*` at the
  collision hit point along its normal; red damage-direction arcs; red edge vignette below low HP
  that pulses faster as health falls; hit marker, kill feed and centre kill notice.
- **Coverage** (headless run log): surface sets 35/35, voices 13/13, misc cues 16/16, zitem
  `snd_fire/reload/dryfire` 19/19 (`swing`, the one name without a file, is aliased to
  `blade_swing`: **inferred**, every other melee item and `animationevent.xml` `melee_attack` use
  it), weapon items with a fire sound 155/155, map ambiences 2/2 (Mansion) and 3/3 (Dungeon)
  resolve and decode.

Other names the HUD plays (the mapping is **inferred** from the names): `hitbody00` (bullet
hit), `blade_damage` (melee hit), `fx_myhit` (type 3, the player's own hit), `death01_a_male`
(death), `fx_respawn` (type 1, player respawn).

### Music (`src/music.rs`; track choice **inferred**, files **observed**)

**Observed**: `sound/bgm/` holds 16 files: `el-tracaz`, `fin`, `gunzmatching` (the only mp3; needs
bevy's `mp3` feature, enabled in `Cargo.toml`), `hardbgm(d)`, `hardbgm3 vanessa retake(d)`,
`hardcore(d)`, `hardtech(d)`, `industrial technolism`, `intro retake2(d-r)`, `league`,
`leagueloop`, `ryswick style`, `theme rock(d)`, `trance mission_tmix`, `vague words`,
`x-fighter`. Their stems appear **only** in `system/filelist.xml`: a grep of the whole extract
(`system/*.xml`, `interface/`, every map `.rs.xml`, `quest/`, `challengequest/`) finds no other
reference, so no retail map/mode-to-track mapping is readable (`Gunz.exe` is packed). The
options screen (`interface/default/option.xml`: `BGMMute`, `BGMVolumeSlider`; strings "Background
Music", "Volume of Background Music") shows the retail game had a mute and a volume.

**Inferred rule** (names only):

| situation | track |
|---|---|
| main menu | `gunzmatching` ("matching" = lobby) |
| match, any mode but the two below | one of the 10 pool tracks (all but the 5 named here), by FNV-1a of the map directory, so a map always gets the same track |
| Duel, Duel tournament | `leagueloop` (`league` is its un-looped twin and is unused) |
| Quest (incl. challenge quest/survival) | `trance mission_tmix` ("mission") |
| match over (`Clock.over`) | crossfade into `fin` (the one short, 130 kB track), played once |

Looped tracks loop (`PlaybackSettings::LOOP`); any change of wanted track crossfades linearly over
2 s (**inferred**). `Settings.music` (0..=1, default 0.5, **inferred**) scales every music voice.
A track is probed (first second decoded) before it is queued; one that bevy cannot decode is
warned about and skipped. Headless `--shot` runs log the choice and play nothing; with no audio
device bevy logs a warning and the sinks never appear. Check: `GUNZ_GAME=<install dir> cargo test
--release every_bgm -- --nocapture` decodes all 16 files completely.

**NPC / animation-event sounds**: `animationevent.xml` names sounds as `quest/<monster>/<File>` or
a plain stem (97 distinct, all resolve in `sound/effect/`, **observed**; stems are unique across
`sound/`). `Cue::Anim("quest/goblin/Goblin_die")` (follows an actor) and the
`game::PlaySound { stem, at }` message (a position) both play them; the directory part is dropped
and case ignored.

## Actors: animations and movement (`src/actor.rs`, `src/bin/gunz-play.rs`)

No new file format; how the retail character animations are used (**observed** = read from
`model/man/man01.xml`, **inferred** = chosen to look right, nothing in the data fixes it).

- Every motion type has whole-body clips only (no upper/lower split): `idle run runB`
  (loops), `jumpU jumpD`, `tumbleF/B/L/R`, `jumpwallF/B/L/R`, `die..die4`, `damage*`,
  `blast*`. Gun types (2-5, 9-11) add `attackS` (one shot, holds its last frame) and `reload`;
  melee types have `attack1..4` (1, 12, 13) or `attack1..2` (14, 15) (+ `_ret`), `attack_Jump`,
  `uppercut`, `guard_*`; dagger (7) only `attackS`. `runW*`/`runLW`/`runRW` are wall runs
  (**inferred** from the names), not strafes: retail has no strafe clip.
- A one-shot clip ends in its last frame (`motion_loop_type` `lastframe`/`onceidle`).
  Attack clips are short: `attack1` of the katana lasts about 0.28 s (**observed**).
- ELU characters face +Z; actors face -Z at yaw 0, so the model child is turned by half a turn.
- Constants (**inferred**; `npc2.xml` NPC `speed` is 400..840 cm/s): run 6.3 m/s, backwards
  x0.7, jump 7 m/s, gravity 22 m/s^2, tumble 9 m/s (double-tap of a direction within 0.3 s),
  wall kick 4.5 m/s away plus 6.5 m/s up (jump within 0.15 s of touching a wall in the air),
  capsule radius 0.35 m / height 1.75 m.
- Steps (**observed** with a probe over Dungeon, Castle, Catacomb, Mansion, Prison: scan col
  geometry for risers, walk 1 m into each at 60/144/240 fps with the controller's 60 m/s^2
  acceleration): retail risers are mostly 0.29-0.31 m (Mansion, Catacomb), 0.40-0.45 m
  (Castle, Dungeon) and up to 0.55 m; ledges of 0.9 m and more are the waist-high ones. The old
  `STEP` 0.3 failed: Catacomb 0/52 risers climbed, Mansion 0.30 m 24/224 at 144 fps. Causes:
  (1) `STEP` below the stairs, (2) the step only counted with >1 cm progress per frame and a
  walker that hit a riser has lost its speed, (3) a sphere resting on a riser's top edge has a
  tilted contact normal (`normal.y` < 0.7), which was not "floor". Now `STEP` is 0.55 m, the
  step probes at least 0.1 m ahead, and an edge contact stands when the triangle is a floor and
  `normal.y` >= 0.3. Result: Catacomb 46/52, Mansion 0.30 m 304/332, Castle 0.40-0.45 m
  ~120/125, frame-rate independent; 0.9 m+ ledges stay blocked.
- Wall run (**inferred** from the clip names `runLW/RW/W` + `_down`, `runW_downF/B`): in the air
  with forward held, touching a wall and at least 0.5 m above the floor, once per jump. Side
  wall: runs 1 s at 6.3 m/s along it with 12 % gravity, then `*_down` (45 % gravity, fall
  speed <= 6 m/s); facing the wall: climbs at 5 m/s fading to 0 over 0.7 s (`runW`), then
  `runW_downF` (`runW_downB` when turned away). Jump during a wall run = wall kick.
- Guard (**observed** clips `guard_start`, `guard_idle`, `guard_block1/2`, `guard_cancel`;
  melee motion types with them only): right mouse / script key `guard` on the ground. The actor
  carries `Guarding` while raised (movement locked); combat writes `Blocked(entity)` to play a
  block clip. Camera: swept sphere (0.25 m) instead of a ray, and it ducks under low ceilings.
- Weapon use: shots at `delay` ms; magazine / reserve from `magazine` and `maxbullet - magazine`
  (**inferred** that `maxbullet` counts the magazine); a reload lasts the character's `reload`
  clip (`load` for grenade/item types): 1.33 s 1h pistol/SMG, 2.0 s the others (**inferred**:
  `reloadtime` 3..10 is not seconds); a melee blow lands at the clip's sword-hand-tip frame
  (`melee.rs` `STRIKE`, **inferred**; clips without one at 45 % of their length).
- Layering (`Animator` from `src/anim.rs`): legs play `idle/run/runB/jump*`, the upper body plays
  `attackS`/`reload`/switch clips while moving, so shooting or reloading while running keeps the
  legs running; clips cross-fade (`blend_for`, 0.05-0.12 s, **inferred**: the data has no blend
  times); the spine takes the aim pitch. Tumbles and wall moves travel at the speed derived from
  the clip root motion (Fidelity), not a free constant.
- Action API (`game::ActionRequest`, `Acting`): other modules ask for a full-body clip with
  speed, root motion, movement lock and cancel window without touching `actor.rs`; `Acting.time`
  lets melee time its hit frame. A `Push` with vertical part >= `BLAST_PUSH` (5 m/s, uppercut,
  rocket, grenade) plays `blast` -> `blast_fall` -> `blast_drop`, lies `LIE` 0.35 s, then
  `blast_stand`; when the same frame's damage came from a dagger the `blast_dagger` (0.5 s) and
  `blast_drop_dagger` (0.667 s) variants play instead (**inferred** use: the clips exist in
  every motion type, are shorter and travel 0.2 m instead of 0.6 m, so they fit a lighter
  launch); ordinary damage plays `damage`/`damage2`. Landing plays `jumpD`. All clips of both
  sexes are parsed at startup (`ActorData`), never inside a match.
- Emotes (**observed** names, every motion type has them): `taunt` (`T`, script `taunt`),
  `bow wave cry laugh dance` (keys F5-F9 **inferred**, script `bow` .. `dance`; `Intent.emote`).
  Standing, free actors only; the `loop` ones play one cycle (`bow` 2.5 s, `wave` 2.67 s, `cry`
  and `laugh` 2 s, `dance` 5.67 s); a jump, dash or step after 0.5 s (`TAUNT_CANCEL`,
  **inferred**) ends them. The log line `clip NAME (motion N) SECSs` names every action clip
  the player starts.
- Run playback rate = ground speed / toe speed of the clip (`stride`): guns 3.8 m/s (rate 1.66
  at 6.3 m/s; the old cap 1.5 slid 10 %), katana 5.64, sword 5.18, dagger 5.98, medikit 4.6,
  backwards 4.8 (**observed**, `.local/py/stride.py`; the cap is now 1.7).
- Clip coverage (`.local/py/coverage.py`: every quoted name in `src/` against the 71 distinct
  `<AddAnimation name>` of `man01.xml` + `woman01.xml`): **63 of 71** are referenced (was 55).
  Unreferenced: `login_intro`/`login_idle`/`login_walk` (`gm="0"`: the lobby/character-select
  pose, not in-game clips); `runW_down` (only motion types 7 and 14, next to the
  `runW_downF/B` pair the wall run plays); `stun`, `lightning`, `bind`, `pit` (`gm="1"` status
  poses; nothing in the data starts them for a player: `bind`/`pit` appear in no XML, `stun`
  is only the tip text of the Spy stun grenade in `messages.xml`, `lightning` is the player
  twin of the NPC `*_damage_lightning` clips). A module that adds one of these effects asks
  for it with `ActionRequest { clip: "stun", .. }`; looping ones play until replaced.
- Death camera: the corpse keeps its rotation (only living actors follow the camera yaw); the
  camera orbits it (`DEATH_ORBIT_PERIOD` 14 s, `DEATH_DIST` 3.8 m) plus the mouse.
- Frame times: `GUNZ_FRAMETIMES=1` (`src/perf.rs`) prints hitches and a p50/p99/max summary.
  Mansion, 3 bots, 60 s headless: play phase max 22.1 ms, 0 frames over 33 ms (the 50.7 ms
  frames are the first two, loading).

### Status effects (`game::Status`, `game::Afflict`, `actor.rs` `status` and `drive`)

**Observed** (`system/zskill.xml`, 49 `SKILL`): `mod.speed` is below 100 on three skills, all with `hitcheck`
false, `effecttype` 0 and no `effectarea`: Slow 151 and 165 (50, `effecttime` 7000, reuse 20 000 ms; 151 is the
goblin chief's, 165 is cast by nobody) and Stun 351 (65, `effecttime` 3000, reuse 15 000 ms,
`castingpreeffect="ef_stun"`; the Unholy 35 / 145 / 175). `mod.root` is true on 14 (Massive Swing 161 163 167
171 173, golem and Unholy missiles 261-263 and 381, blizzards 431 432 441 442 451; only the Swings have an
`effecttime`, 1000), `mod.dot` is 30 / 35 on the five blizzards (every other `mod.dot` is 0), `mod.antimotion`
is always false. `man01.xml` has a `stun` clip (`man_stun`, loop, every motion type) that nothing used.

**Model** (**inferred**; the data names no rule): one message, `Afflict { target, by, secs, slow, stun, root,
dot }`, from an NPC skill (`npc.rs` `afflict`), a frost bullet or a stun grenade; `actor.rs` merges it into the
target's `Status` (the stronger slow, the longer of each timer), ticks it and removes it when it ran out or
the actor died. Effects last `effecttime` (a root without one: 1 s, like the Swings').

- **Slow**: run speed x factor (the run clip follows the ground speed). **Stun**: the `stun` clip plays for
  as long as the status; no walking, jumping, tumbling, shooting, slashing, reloading, guarding or weapon
  switching (`Intent` is zeroed but the look); the stun skill's 65 % speed rides along and changes nothing.
  **Root**: no walking, jumping or tumbling, attacks stay allowed. A stun roots too. Effects: `ef_stun` / `ef_slow_dam`
  at the victim.
- **Damage over time**: `mod.dot` is extra damage spread evenly over `effecttime` and paid every 0.5 s
  (a blizzard: 30-35 on top of the hit, over 3 s). The other reading, `dot` per second, would add 90-105
  and kill a 150-point player from one cast.
- **NPC use** (`npc.rs`): a skill with no area (151, 165, 351) hits the caster's target when it is in sight
  within 12 m (`STATUS_RANGE`), a missile or area skill afflicts what it hits; the cone / disc hit also
  applies slow / root / dot. Heal skills are those with `effecttype 6` (the blizzards carry a `mod.heal` too, which
  made them heal spells before).
- **`pierce`** of `zactoraction.xml` (MELEESHOT 0, RANGESHOT 50, GRENADESHOT 0 per cent) is now the
  `Damage.pierce` of NPC blows, shots and grenades, so armour soaks claws and blasts and half of a bullet;
  `None` (players, skills) keeps the weapon's own value.
- **Bots**: a rooted or stunned bot resets its stall check (it is not "stuck", no nav link is marked broken)
  and replans when it is free; slowed bots just walk slower. Stunned bots play `stun` like the player.

Checks (Mansion, `--bots 0 --hp 400 --ap 200 --script "wait:12;w:3;wait:10"`, `RUST_LOG=gunz::actor=info`):
`--npc 35` logs `status: Player <- slow x0.65, stun true, root false, dot 0 for 3.0 s` and `clip stun (motion 1)`;
`--npc 15` (goblin chief) `slow x0.50 ... for 7.0 s`; `--npc 44` (palmpou) `root true, dot 35 for 3.0 s` and
six 6-point `damage` lines 0.5 s apart. With `--bots 2 --skill 1 --npc 145,15 --bots-ahead 10` the bots are
stunned and slowed, keep fighting and kill both (`RUST_LOG=gunz::bot=debug`: no `stuck` growth).
- **HUD** (`hud.rs`, `Label::Status`, above the HP bar): one line per running effect of the player with the
  seconds left (`STUNNED 1.8s`, `ROOTED`, `SLOWED 50% 5.8s`, `BURNING 2/s 2.5s`), blue text. **Observed**: the
  retail `interface/default/` has no status icon (`ingame_statusboundary.png` is a 4x72 frame strip,
  `buffevent_0x.png` event banners), so labelled text stands in. `zbuff.xml` buffs stay unwired (nothing
  references an id, see below), so the HUD shows none. Check: `GUNZ_AFFLICT="24.8:stun,24.8:slow"` (afflicts the
  player at that match second, kinds `stun|slow|root|burn`) with the Spy command of that section gives
  `.local/shots/SpyMore/ping.png` (stunned and slowed).

## World items (`src/pickup.rs`)

**Observed**: 23 of the 31 maps' `.rs.xml` carry `spawn_item_{solo|team}_{hp|ap|bullet}NN_MM` dummies, 243 in
all, e.g. Mansion 7 (5 solo, 2 team), Town 6 (4 solo, 2 team), Citadel 19; every one's `{hp|ap|bullet}NN`
is a `system/worlditem.xml` `WORLDITEM@name` (243/243). Fields: `TYPE` hp|ap|bullet, `AMOUNT` (hp01 10,
hp02 25, hp03 50, ap01 10, ap02 25, ap03 50, bullet01 1, bullet02 2), `TIME` ms until respawn (hp01/ap01/
ap02/bullet01 3 s, hp02/bullet02 5 s, hp03/ap03 10 s), `MODELNAME` red/green/yellow ->
`MeshInfo/AddWorldItemElu` (`model/worlditem/ef_<name>.elu` + looping `.elu.ani`, the spin/bob; every item
also gets `baseEffect` = `ef_prop.elu`). Solo dummies are used by non-team modes, team dummies by team modes
(`Mode::team_items`); no pickups in duel/training.
**Inferred**: touch = actor feet within 0.85 m horizontally and -1.75..+0.3 m vertically of the dummy;
an item is only taken when it helps (hp/ap below max, a gun reserve below `maxbullet - magazine`);
`bullet` adds `AMOUNT` magazines to the reserve of every gun; items refill on `NewRound`; pickup sound
`sound/effect/fx_itemget.wav`. Bots/others read `WorldItem`+`GlobalTransform` (`useful_for`).
Check (Mansion, hp 40, walking onto `hp02`): `pickup Hp +25 ... hp 65`, `respawn Hp +25` 5 s later.

## Combat and bots (`src/combat.rs`)

No new file format. **Observed** = read from retail data, **inferred** = chosen (not in the data,
the executable is packed).

- **Damage numbers** (**observed**): `damage` of the item per shot (shotgun: per pellet, summed per
  target into one `Damage`), melee `damage` per slash; `delay` and `reloadtime` are applied by the
  actor controller. `Damage.item` names the weapon. HP/AP rule: `absorb(v, amount, pierce)`:
  `pierce` of the hit goes to HP, the rest to AP, what AP cannot hold falls through to HP.
  `pierce` is **observed** only for NPC attacks in `system/zactoraction.xml`: all 225 `RANGESHOT`
  `pierce="50"`, all 80 `MELEESHOT` and 62 `GRENADESHOT` `pierce="0"`. Applying those to player
  weapons is **inferred** for every class (no player-weapon attribute exists): blades 0,
  rockets/frags 0, every gun 0.5. Earlier inferred values were blades 0.7, rifle/MG 0.6, shotgun
  0.3, rocket 0.5, other guns 0.5; `piercing` in
  `combat.rs`. `Vitals` of actors are 100 HP / 50 AP from `actor.rs`. Self damage is
  only taken from your own blasts; HP <= 0 inserts `Dead{respawn: 5 s}` (**inferred**), a
  suicide adds a death but no kill. `Protected` actors take nothing (no damage, blood, push).
  Quest monsters carry `game::HitShape { radius, height }`; hitscan, rocket bodies and blasts use it
  instead of the 0.35 x 1.8 m human capsule (`combat::shape`).
- **Guns**: hitscan from the `Fire` ray against the map (`MapCollision::raycast`, nearest first) and
  a vertical capsule per actor (feet at the transform, radius 0.35 m, height 1.8 m); range 200 m
  (no range attribute for guns; **inferred**). Spread (all **inferred**; `ctrl_ability` is 10
  pistol, 15 rifle, 20 SMG/revolver, 35-80 dual guns, 60 shotgun/MG): cone radius per metre =
  `ctrl * 0.001 * (1 + 1.5 run + 1.0 air + 2.0 heat)`, `run` = horizontal speed / 6.3 m/s,
  `air` = vertical speed above 3 m/s (both smoothed, sampled from the actor's `GlobalTransform`
  by `track_spread`), `heat` builds `ctrl * 0.01` per shot and cools 1/s (a rifle at 13 shots/s
  saturates in ~1 s; a pistol never heats up). Measured with the rifle (ctrl 15): standing
  0.0150/m on the first shot, 0.0290/m after 8 shots; running first shot 0.0374/m (2.5x), 8th
  0.0515/m; first shot just after jumping 0.0173/m, 0.0293/m in the air. Shotgun: 12 pellets
  (**inferred**, zitem.xml has no pellet count; 13 damage per shot would be useless otherwise),
  each in a cone 2x wider than a bullet (0.12/m = 1.2 m radius at 10 m): chest-aimed on a bot
  3 m away 12/12 landed (156 damage, a kill), at 10 m 5..11 of 12 (the 0.7 m wide body). Dual guns
  alternate hands, right first (`combat::Hand`, read by the actor controller; the muzzle flash
  comes from that hand's node). Pistols, revolvers, shotguns and launchers need one click per
  shot; `WeaponKind::automatic` (SMG, rifle, machine gun) fires while held (**inferred**).
  Reload lasts the `reload` clip length, 1.3-2 s (**inferred**; `reloadtime` values pistol 4,
  SMG/shotgun 5, rifle 6, revolver 8, MG 10, rocket 3 have no consistent ratio to the clips and
  8-10 s reloads contradict play): measured revolver `reload: ... 1.3 s` at t=2.50, `reload done`
  at t=3.85; rifle 2.0 s (t=2.50 → 4.52). A weapon
  switch costs `SWITCH_DELAY` 0.3 s (**inferred**), cancels a running reload (ammo untouched) and
  shot recovery. Each shot of the player logs `shot: item .. spread ../m, N landed`.
- **Projectiles and consumables** (`src/projectile.rs`): rocket = projectile (no rocket ELU exists in
  model/weapon; capsule mesh + `ef_rocket_smoke.elu` trail, 30 m/s, 3.5 m splash, **inferred**),
  explodes with `rocket_effect`. Frag/flashbang/smoke: model `model/weapon/grenade/*.elu`, released
  0.3 s after the throw (clip `attackS` of motion 6), 10 m/s, gravity, bounce, fuse 1.5 s; radius =
  `handweaponcolldist` cm (**observed**), flash/smoke duration = `handweaponstatetime`
  (**observed**; reading is **inferred**). Splash falls off linearly with capsule distance, walls
  shield; `Push` = 9 m/s away + 8 m/s up at the centre, scaled by the falloff (the actor
  controller launches into the blast clips from 5 m/s up). The owner takes part of their own blast
  (`SELF_BLAST` 0.5, **inferred**) and is pushed (rocket jump): a rocket fired at the floor 1 m
  ahead logs `blast: Player -> Player 35 (0.8 m of 3.5 m)`. A flashbang blinds every actor in
  range with line of sight (`Flashed`, bots included; bots must react to it). A smoke grenade
  also spawns a `SmokeCloud` sphere (radius 0.6 x `handweaponcolldist`, lifetime
  `handweaponstatetime`) for `projectile::smoke_blocks`, a map-independent sphere/segment test for
  line-of-sight checks. Medikit/repair kit: `system/worlditem.xml` `AMOUNT` of the entry named like
  the item's `mesh_name` (**observed**: medikit 30, `medikit_B` 45, `medikit_C` 100, repairkit 35,
  `_B` 55, `_C` 120; `Weapon::kit_points`); potions `itempower` per second for `damagetime` s (**observed**).
  Effect ELUs are one-sided sprites facing +Z: they are turned to the camera (`Vfx::Facing`);
  muzzle flash uses the `muzzle_flash` node frame (local +Y = barrel, **observed**) and only the
  weapon in hand (dual guns: only the firing hand's node, see `Hand`). No hit-box/headshot
  multiplier exists in the data: capsule kept.
- **Blades**: items have `range` (cm, 120..620) and, for the legacy NPC items only, `angle` (30..60).
  A slash hits every actor whose capsule is within `range` of the chest (1 m above the feet)
  horizontally, within +-1.2 m height, inside a cone of `angle` degrees (**inferred** default 90
  when absent) around the facing, with no wall between.
- **Knockback** (**observed** table, **inferred** unit): `system/zeffect.xml` `<EFFECT id knockback>`
  (ids 3-11: pistol 30, SMG 20, shotgun 400, rifle 50, machine gun 150, revolver 100/200) is
  indexed by the weapon's `effect_id`; read as cm/s of horizontal velocity (`game::Push`, m/s).
- **Weapon limits** (**observed** attributes): `limitspeed="90"` + `limitwall="1"` on exactly the 12
  rocket and 11 machine-gun items (weights 30-50), nothing else. UI strings `messages.xml` 9314
  "Speed", 9315 "Disabled : Jump", 9316 "Disabled : Dash", 9317 "Disabled : Wall Climb" (the item
  tooltip rows; only the first and last have a zitem attribute). Read as: 90 % of the run speed while
  the weapon is in hand (percent unit **inferred** from 90) and no wall run / wall kick
  (`Weapon::limit_speed`, `limit_wall`; `actor.rs` `Gear`). `weight` (0-50) and equipment `maxwt`
  (0, one item 10) are the inventory capacity rows 9304 "Weight"/9313 "Max Weight": no movement
  attribute uses them, so weight does not slow anybody.
- **Not data** (searched, nothing found): `zbuff.xml` has 7 `<BUFF id Period EffectType="dote" hp|ap>`
  (40100-40103 hp 100/3/3/3, 40301-40303 ap 3, `Period` 8) but no item, NPC or skill references an
  id, so they cannot be wired (**unknown** consumer); `hppercentformula.xml` is only an exp/bounty
  bonus per hp percent (50-90) per game type, no damage rule; `system.xml` only has report/locator
  settings; `gametypecfg.xml` only round/time/player menus; no gravity, jump, fall-damage, guard
  window, uppercut, massive, switch-time or pickup-radius value exists. `blitzkrieg.xml`
  `<RESPAWN baseTime="8" invincibleTime="5"/>` (+ message 2119) is the only respawn/protection
  number, Blitzkrieg only. NPC grenades carry `force` 800-1500 (cm/s, unit **inferred**), the
  range our 10 m/s throw sits in; NPC `collision.radius` 40-120 / `height` 115-200 cm bracket the
  player capsule 0.35 x 1.75 m without fixing it; world-item respawns (`TIME` ms) are already read.
- **Effects**: muzzle flash = `effect_list` `flame_pistol` (pistols/revolvers), `flame_rifle`
  (SMG, rifle), `flame_mg`, `flame_shotgun`, spawned at the weapon ELU's `muzzle_flash` node
  position, oriented along the aim ray at half size (**inferred**: the node's own frame does not
  line up with the barrel: the flash came out as a vertical spike). Blade hits spawn
  `sword_damage1..3`. Retail has no blood/spark ELU in `effect_list.xml`; the textures
  `sfx/blood01..05.tga` (red blobs, alpha) and `sfx/ef_gz_spark.bmp` (additive spark spray) are
  drawn as camera-facing fading quads (blood on actors, spark on walls; **inferred** use).
- **Bots** (`src/bot.rs`, `src/nav.rs`; not data driven): see "Bots" below.

### Bots (`src/bot.rs`, `src/nav.rs`)

- **Retail `.nav` files** (**observed**): only quest/challengequest maps plus `hall` and `blitzkrieg`
  ship one; no deathmatch map (Mansion, Citadel, Dungeon, ...) does. The format is not decoded;
  the graph below is built from `.RS.col` instead and works on every map.
- **Nav graph** (`nav::Nav`): floors sampled with downward rays on a 0.5 m grid (every hit with
  `normal.y >= 0.7`, 1.75 m headroom, and a capsule `slide_move` that actually lands on it within 8 cm;
  the capsule test drops about 40 % of ray floors: faces the loader keeps oriented *down*, i.e. ceilings
  and undersides of slabs, which `raycast` (two-sided) reports with an upward normal). Mansion 18.9k
  nodes, Citadel 31.7k, battle arena 68.9k. **All links are built at load**, in parallel on every core
  (0.05-3.7 s per map, Mansion 1.0 s, in `PostStartup`), by **simulating the controller** (`slide_move`
  at 1/30 s, run speed, gravity, jump speed and the wall-run table `actor::climb` from `actor.rs`):
  walk 8 directions (steps up to `col::STEP` only), run off a ledge (`Drop`, <= 20 m fall, <= 6.5 m away;
  **inferred** acceptable because the port has no fall damage; 8 m before), jump (`Jump`, ledges and gaps where
  walking failed and nothing is at 1.4 m height) or **wall climb** (`Jump` whose takeoff is the point
  where the run-up first sees a wall within 2 m, on the 4 axis directions: jump so that the jump peaks
  at the wall, the controller's wall run (`runW`, 3.3 m in 0.6 s, `WALL_MIN_HEIGHT` of air under the
  feet) takes over and the wall-run's push into the wall carries the capsule over a ledge up to about
  4.5 m; this is how the 4.5 m balconies of Skirmish Hall and the 9 m levels above them link, 66 -> 100
  of 104 spawn routes) or **side wall run** (`Jump` with a `turn` heading: at a ledge with a wall within
  4 m at the side, the jump turns 0.35 or 0.6 rad into the wall and the controller's *side* wall run
  (`fwd.dot(n) >= -0.7`: `RUN` speed along the wall for `WALL_RUN_SIDE` = 2 s at `RUN_GRAVITY` = 0.12 g,
  entry `vy.min(2)`, **observed** in `actor.rs`) carries it up to 14 m along the wall; this is what
  crosses Mansion's 10.5 m pit in the corridor floor to each wing, which the 8 m `DROP` search could
  not pass; in the air the sim steers with the controller's `approach(hv, wish, 10 m/s^2)`).
  A link a bot is stuck on twice is marked broken and replanned. Jump steps carry the simulated
  takeoff point (and, for a side run, the heading and the seconds to hold it); the bot runs to it,
  jumps, keeps that heading in the air and then walks on to the landing node.
- **Searches** (`Nav::search` / `Nav::advance`): A* kept as a resumable `Search`; every frame one bot may
  start a replan and all running searches share a budget of 2500 node expansions (about 2 ms), so a
  long route spreads over a few frames while the bot keeps following its old path. Before: links were
  built inside the search and the first searches took 0.1-22 s (worst per map, dev probe); now the
  worst whole search on any of the 29 maps is 4.1 ms.
- **Behaviour** (inferred): target = nearest non-friendly actor (`game::friendly`: without `Team`,
  bots only target the player and never hurt each other; with teams, the other team); route replanned
  every ~1 s, walking to the farthest of the next 4 nodes that is a straight walk; weapon by range
  every >= 1 s (blade < 3.5 m, else the gun with ammo whose best range (shotgun 5 .. rifle 16 m) is
  nearest); strafe/hold distance, hops and double-tap tumbles in fights; below 15 % + 30 % x skill HP
  they run to the spawn point farthest from the enemy until it is within 3 m; `BotSkill` 0..1
  (default 0.5) scales reaction (0.6-1.2 s x (1.5 - skill)), aim error (+-5 deg x (1.5 - skill)), turn
  rate, hop/tumble rates, the retreat threshold and the odds of the moves below. Idle bots wander
  between spawn points. `RUST_LOG=gunz::bot=debug` prints per-bot state every 2 s.
- **GunZ moves** (inferred, `bot.rs`): a blade bot guards when an enemy blow starts (`Acting` clip
  `attack*`/`slash`/`uppercut`/`jump_slash*` in its first 70 %; chance 30 % + 60 % x skill per swing, guard
  0.7 s), answers with `attack` while guarding (the guard's uppercut) once the blow is over, holds
  `attack` against a guarding enemy (the held click charges a massive swing, which ignores the guard),
  and **K-style dashes**: 3-8 m from an enemy it double-taps forward and slashes from 0.35 s into the
  dash. Flashbang (`projectile::Flashed`): the bot sees nothing, stumbles at half speed, never attacks
  or jumps; smoke (`projectile::smoke_blocks`) and walls cut its sight. A bot short of health (< 70 %),
  armour (< 50 %) or ammo (a gun without spare magazine) walks to the nearest ready `WorldItem` of that
  kind within 40 m (>= 6 m from the enemy), giving up after 15 s. Gun bots hold fire while a friend is
  within 0.7 m of the line to the target.
- **Grenades** (inferred, `bot.rs`): bots carry the default loadout plus a frag (zitem 2200007, slot 3);
  a bot without a frag or flashbang with ammo skips this. With the enemy 5-14 m away (out of melee range)
  and either behind cover (no sight line) or grouped (>= 2 enemies within 0.8 x the blast radius of
  the target), at most one plan per frame and one per second per bot (10-16 s after a throw, 3 s after
  spawn), the bot plans the throw: pitches -0.3..1.1 rad are flown through `grenade_landing`, the
  same step as `projectile::fly` (gravity, 0.1 m sphere sweep, bounce, friction, fuse), and the pitch
  whose landing is within 60 % of the blast radius of the target's chest and more than the radius
  from the thrower wins. Then it equips the grenade (`Intent::slot`), turns to the enemy, clicks once
  (`Intent::attack` for a frame, at the planned pitch, re-planned at the click) and stands still until
  the fuse is out; the throw itself is `projectile::launch`, exactly the player's path.
- **Smoke** (inferred, `bot.rs`): bots carry a smoke bomb (zitem 2200002, slot 4) beside the frag. A
  bot below 30 % + 30 % x skill of its health that sees its enemy at 4-20 m throws it (at most once per
  10-16 s): `plan_throw` picks the pitch that lands it within 1.8 m of the point 4 m towards the enemy,
  so the cloud (`projectile.rs`: 0.6 x the item's radius, `state_time` s) stands between them and
  `smoke_blocks` cuts the sight line; it holds still only for the throw delay + 0.3 s, not the fuse.
  Check (Mansion, `gunz-play GAME Mansion --at -2430,-3150,5 --yaw 270 --hp 400 --ap 200 --bots 3
  --bots-ahead 9 --script "3:0.1;attack:0.75;wait:10"`, `RUST_LOG=gunz::bot=debug,gunz::projectile=info`):
  `damage: Player -> Bot 2 ... hp 52 -> 30`, `bot Bot 2: smoke slot 4 at 11.5 m (hp 6)`,
  `t=5.35 detonation: Smoke of Bot 2 at -18.3, 0.2, -30.6` (`.local/shots/BotsMansion/smoke.log`).
  Perf with 8 bots (`--bots 8 --mode tdm --script wait:40`, `GUNZ_FRAMETIMES=1`): `[perf] play:
  frames=2459 p50=16.7 p99=16.8 max=222.0` (one load hitch, the graph build is 1.0 s).
- **Butterfly** (inferred, `bot.rs`): after its own `attack1..4` has reached `Acting::cancel_from` (the
  hit frame), a melee bot rolls 15 % + 50 % x skill once per blow and raises the guard for 0.1 s
  (`melee.rs` takes it as the guard cancel of the recovery, and the 0.1 s ends before the weapon's next
  blow is ready, or the held click would turn into the guard's uppercut), then the held `attack`
  starts the next combo blow. The existing guard against incoming slashes is unchanged.

Verification (headless, `.local/shots/bots/`, all `gunz-play GAME MAP ... --time T --shot`):
- Mansion `--at -970,1310,593 --bots 4 --time 30`: bot from the y=13 floor logged
  `pos [12.1,13.0,19.9] -> [9.8,8.7,18.2] (drop) -> [-1.3,8.0,17.7] (jump) -> [-4.2,3.3,13.6]` and shot the
  player on the y=6 landing at t=5.1 (3 bots, kill at t=6.8); a ground bot climbed the ramp
  (`[2.9,0.1,5.3] -> [1.1,1.3,7.7]`). `foot_t4.5.png`: a bot jumping off the upper flight, `down_t4.png`,
  `down_t6.png`: bots running at the stair foot and shooting.
- 6 bots, 100 s, player standing still: Citadel 4 player kills, Dungeon 2 (bots reached y=22 floors),
  Castle 2; 0 `Bot -> Bot` damage lines in FFA on all four maps; TDM on Dungeon: bots on different
  teams hurt each other.
- Retreat: rifle-shot bot at hp 6 fled 8 -> 52 m to a far spawn and swapped to the rifle; weapon log
  `slot 0 -> 2 at 39.8 m`, `slot 2 -> 1 at 8.0 m`, `slot 1 -> 0 at 4.8 m`.
- Grenades and butterfly (`gunz-play GAME Mansion --at -970,1310,593 --bots 8 --mode tdm --time 90`,
  `RUST_LOG=gunz::bot=debug,gunz::projectile=info,gunz::melee=debug`): `bot Bot 5: grenade slot 3 at
  8.5 m (behind cover)`, `bot Bot 7: grenade slot 3 at 7.9 m (grouped)`, then `detonation: Frag of Bot 5
  at 3.7, -7.0, -19.7` and `blast: Bot 5 -> Bot 4 23 (2.7 m of 4.0 m)`; `bot Bot 8: butterfly after
  attack1 at 0.17s` followed by `melee Bot 8: slash strikes at 0.183s` and `melee Bot 8: guard Start
  (guard_start)`. `[perf] play: frames=5459 p50=16.7 p99=16.7 max=16.9 >20ms=0` with the 8 bots
  (`GUNZ_FRAMETIMES=1`).
- Mansion routing (`nav::tests::routing_pairs`, `GUNZ_MAPS=mansion`): 53/142 before, 124/142 after. The
  real reason the wings were missing: the corridors that lead into them (west door at z -31.5..-28,
  x -18; the mirror one on the east side) have a 10.5 m wide pit in the floor (y=6 floor from x=-17.5
  to -28 missing, y=0 courtyard 6 m below, a roof at 12.7), not a shaft too deep to drop into; the
  old controller model only knew the *up* wall run (facing the wall), never the side one that the
  controller grants at a glancing angle, which crosses 12.6 m. In-game (`gunz-play GAME Mansion --at
  -970,1310,593 --bots 4 --bots-ahead 11 --time 60`, `RUST_LOG=gunz::bot=debug`) all four bots logged
  `side wall run from [-18.0, 6.0, -28.3] heading [-0.94, 0.00, 0.34]` and were in the west wing 4.5 s
  later (`pos [-43.1, 6.0, -22.9]`); a 3-bot run with the player in the wing (`--at -5500,-1290,610`)
  ended with a bot killing the player there at t=19 s.
  The y=13 floors (6 spawns; most of the 18 remaining failed pairs) are **not** a nav link, though the
  player can reach them. Numbers (**observed** controller constants of `actor.rs`, replayed in a
  throwaway `Pawn` at 1/60 s): wall run up 3.3 m + 0.45 m coast, `n * WALL_OUT + Y * WALL_UP` kick = +0.96 m, a kick clip lasts
  1.0-1.33 s (`man_jump_wallB` 30 frames, `jump_wallL/R`, `run_wall_down` 40, at 30 fps) during which no
  second kick is possible, and the wall run is once per jump): the best height gain from a floor is
  about 5.8 m, the lip is 7.0 m above the y=6 gallery, so no wall/pillar of the hall (double wall kicks
  between pillars are impossible: 1.0 s per kick falls 4.5 m) and no prop (statue tops reach y=8.5, 10 m
  from the shelves) gets there. A 16-direction beam search over the controller (150-500 states, 0.1 s
  inputs, from 348 floor nodes y 5.5-8 in x,z +-32) found exactly one way: the north platform
  (`-9.8, 6.0, -26.8`), outside the hall's north wall z=-24. Run at yaw 67.5 deg (toward +x,+z), jump
  0.1 s in, wall-run up the corner of the pillar at x=-8.7 (up to y=10.9), kick at ~1.4 s after the
  jump (y=10.85), land on the pillar capital `(-8.3, 11.7, -25.3)`, walk its moulding steps (12.2,
  12.9) onto the y=13 floor at `(-9.4, 13.0, -27.3)`, which links to all six y=13 spawns. Real
  controller check (`gunz-play GAME Mansion --at -980,-2680,600 --yaw -157.5 --script
  "yaw=-157.5;w:0.1;yaw=-157.5;w+jump:0.017;w:0.083;...;yaw=-90.0;w+jump:0.017;w:0.083;..."`, 34 steps,
  `.local/shots/BotsMansion/script1.txt`, shot `climb2.png`): feet `y 6.0 -> 7.1 (run starts, t=0.4 s) ->
  10.86 -> kick -> 11.72 -> 13.00`, standing at `(-9.38, 13.00, -27.34)`. It is **not** usable by bots: the
  contact is the pillar's corner (the wall normal is slanted `(-0.25, -0.97)`), a takeoff 0.06 m
  earlier/later or 0.05 m aside loses the wall run before the kick, and a search for an aim point
  robust to those misses found none (Mansion routing stays 124/142). A link kind that fragile
  was not added; the experiment is kept in `.local/shots/BotsMansion/nav_kick_experiment.rs.txt`.
  The nav graph's wall climb link still reaches 4.4 m.
  Battle arena: 8 spawns sit in 6 m deep pits; Blitzkrieg: the team bases at y=9 have no way up. The
  other failing maps (castle, high haven, island, towns) were not re-searched with the controller beam.
  Tried 0.25 m cells (door alignment): same Mansion result, 4x nodes, 1.7x slower routes; kept 0.5 m.

Routing table (`cargo test --release routing_pairs -- --ignored --nocapture` with `GUNZ_GAME=<install>`;
2 spawn->spawn pairs per spawn point: the next spawn and the one half the list away; a route counts when
it ends within 2 m of the goal; `GUNZ_BAD=1` also prints the failed pairs). before = the graph without
side wall runs and with the 8 m `DROP`, after = this change:
```
map              before      after   | map             before      after
battle arena     111/142    134/142  | prison          142/142    142/142
blitzkrieg        64/80      64/80   | prison ii       140/140    140/140
castle            80/90      84/90   | ruin            146/146    146/146
catacomb           4/4        4/4    | shower room       4/4        4/4
citadel          102/102    102/102  | skirmishhall    100/104    100/104
classic town     132/140    136/140  | snow_town       140/152    144/152
dungeon          140/140    140/140  | stairway         62/88      88/88
factory           86/88      86/88   | station         100/104    101/104
garden            84/84      84/84   | test_a           96/96      96/96
hall               4/4        4/4    | test_b           96/96      96/96
halloween town   140/152    144/152  | town            132/140    136/140
high_haven        59/82      61/82   | weaponshop       64/64      64/64
island            82/92      82/92   | jail              4/4        4/4
lost shrine       76/88      78/88   | mansion          53/142    124/142
port              84/84      84/84   | TOTAL         2527/2794  2672/2794 (90.4 % -> 95.6 %)
```
Of the 122 failed pairs left, most are climbs of 4-9 m (castle, lost shrine, the towns, the stations: a
wall climb reaches 4.4 m) and spawns floating over no floor (High Haven: 12 routes have no start node);
the rest are the pits of battle arena and the bases of Blitzkrieg. Route search is as fast as before
(worst whole route on any map 8.5 ms, build 0.1-5.4 s).

Combat verification (headless, `.local/shots/play/combat/`): `gunz-play GAME Mansion --at -2430,-3150,5
--yaw 270 --bots 1 --bots-ahead 5 --script "3:0.1;attack:0.8" --time T --shot OUT.png` -
`k*.png`/`dead*.png` (blood, bot dying, bot dead, kill feed); the rifle log reads
`t=1.62 damage: Player -> Bot 1 24 (ap 50 -> 26, hp 100 -> 100)` ... `t=2.12 ... (ap 0 -> 0, hp 30
-> 6)`, `t=2.32 kill: Player killed Bot 1`. `approach.png` (3 bots from spawn points, `--time`
4/8/12/16): bot-to-player distances fell 64 -> 31, 43 -> 9, 60 -> 1.7 m between t=2 s and t=14 s.

## Melee (`src/melee.rs`)

No new file format; **observed** = read from retail data, **inferred** = chosen (the executable is packed).

- **Clips** (**observed**, `man01.xml`): per motion type 1 katana, 7 dagger, 12 sword, 13 blade, 14 2h dagger the
  `attack1..4` (+ `_ret` recoveries), `attack_Jump`, `uppercut`, `charge`, `slash`, `jump_slash1/2`,
  `guard_start/idle/block1/block2/cancel`, `damage`, `damage_down`, `blast_*`. Item `damage`, `delay` (ms),
  `range`, `angle` come from `zitem.xml`.
- **Hit frame** (**inferred**): the frame of the fastest sword-hand tip (`R Finger0Nub`, FK on the `.ani` keys) is
  when the blade lands (katana `attack1..4` 5/7/10/10, `uppercut` 8, `slash` 12); no entry = 45 % of the clip.
  Logs show the strike at exactly that frame: `attack1` at +0.167 s, `attack2` +0.233, `attack3` +0.333.
- **Controls** (same `Intent` for player and bots): click = next combo blow `attack1..4` (4th knocks down) with
  `_ret` recovery between; click buffered 0.4 s. `attack` held 0.5 s from the click = `charge` (2 s clip); released
  at >= 0.4 s = `slash` (massive: 160 deg, reach x1.2, +50 % at 1.2 s, throws victims into the blast chain, ignores
  the guard). `guard` held = `guard_start` -> `guard_idle`, blocks frontal (+-90 deg) melee with `guard_block1/2`
  (after `guard_block1` the `guard_block1_ret` return clip plays, motion types 1, 13, 14, 15; `block2` has none),
  `guard_cancel` on release; `attack` while guarding = `uppercut` (launch `Push.y` 7, blast_fall).
- **Cancels** (**inferred** windows): guard during a recovery = butterfly (combo continues for 0.9 s after);
  jump or tumble once the blade has landed = K-style (`attack_Jump` / `jump_slash1` from the air, combo continues 0.9 s).
  Reach 2.55 m, arc 45 deg half-angle for katana slashes; vertical reach 1.2 m.
- Verification (headless, `.local/melee/run.sh NAME SCRIPT TIME --bots N --bots-ahead M --skill 0`, shots and logs in
  `.local/shots/Melee/`): `combo4` (4 hits at +0.17/0.23/0.33/0.35 s of each clip), `uppercut_b` (Push y 7 launch),
  `massive` (`--bots 3 --bots-ahead 5.3`, script `attack:0.95`: three bots hit at t=2.85, all thrown down),
  `guard` (`blocked by Player (guard_block1/2)` for bot slashes), `butterfly` (slash 1.50 -> recover 1.80 -> guard
  Start 1.80 -> Release 2.40), `kstyle` (recover 1.80 -> `K-style cancel` 1.90 from a tumble -> combo continues).

## Animation-derived constants (Fidelity audit; probes `.local/py/{fid,fid2,stride,hit,rootmo,coverage}.py`)

Method: parse `.elu.ani` (axis-angle for 0x12/0x1001), forward kinematics over the Bip01 hierarchy
(upper body reproduces each clip's frame-0 base matrices exactly; leg chain is only checked by the
lowest toe touching the ground, y -0.6..-2 cm), 30 fps. Root motion = `Bip01` + `Bip01 Footsteps`
translation; forward is -Z. All numbers **observed** from keys except where marked inferred.

- Run clips are 20 frames (0.667 s, two steps) and move in place. Speed of the grounded toe against
  the body: knife 5.64 m/s, sword 5.18, 2h dagger 5.98, pistol/rifle/shotgun/grenade 3.8 (4 contact
  frames, low confidence); `runB` 4.80 m/s backwards. There is no strafe clip.
- Tumble (10 frames, 0.333 s), wall kick (`jumpwallF` 1.0 s, B/L/R 1.333 s), `runLW`/`runRW` (2.0 s),
  `*_down` (1.0 s), `runW` (0.6 s) keep the root in place, except `runW`, which climbs 3.3 m
  (feet dummy y 0.16 -> 3.43 m; m/s per frame 6 8 0 1 2 3 4 4 8 15 9 9 10 9 5 3 3). Tumble and kick
  distances are therefore gameplay values (inferred).
- Melee root motion (forward m / clip s): katana attack1 .97/.30, attack2 .76/.30, attack3 1.04/.40,
  attack4 1.25/.80, slash .98/.83, charge .55; returns +.55/+.24/-.81; sword .78 .89 1.31 1.01, slash 1.0;
  blade .74 .85 .98 .59, slash .81; air clips and 2h dagger 0.
- Hit moment = frame of the fastest `R Finger0Nub` (inferred to be the contact): katana a1..a4 frames
  5/9, 7/9, 10/12, 10/24, slash 12/25, uppercut 8/17, jump_slash1 9/39; sword 8/18 10/20 9/22 16/40 slash 13/42;
  blade 5/9 5/9 6/12 11/21 slash 11/25. Full list: `src/melee.rs` `STRIKE`.
- Gun clips: `attackS` 0.167 s (1h pistol/smg), 0.2 s (2h pistol/smg, shotgun, rifle), 0.4 s (rocket),
  grenade 0.5 s with the release at frame 8; `reload` 1.333 s (1h pistol/smg), 2.0 s (the others);
  `load` 0.333 s in every motion type. `zitem.xml` `reloadtime` (pistol 4, smg 5, rifle 6, rocket 3) does
  not match the clip lengths (unit unverified).
- Hit reactions: `damage`/`damage2` 0.333 s, `damage_down` 1.2 s, `blast` 0.667, `blast_fall` 0.667,
  `blast_drop` 0.9, `blast_stand` 1.0, `blast_airmove` 1.5, `die` 1.667 s.

## Profile, shop and ranks (`src/profile.rs`, `src/shop.rs`)

An offline profile replaces the server account. **Observed** = read from the named retail file,
**inferred** = chosen by us (the executable is packed, nothing in the data fixes it). The clan a profile
can hold is described in "Clans" below.

### Data files

- `system/shop.xml` (**observed**, 1 984 lines): not item data but the *layout* of the 800x600 retail
  shop screen (`FRAME`, 23 `PICTURE`, 14 `BUTTON`, 13 `ITEMSLOT`, 4 `EQUIPMENTLISTBOX`, a `CHARACTERVIEW`).
  The equipment slots it names are `Melee, Primary, Secondary, Custom1, Custom2, Head, Chest, Hands,
  Legs, Feet, FingerL, FingerR, Avatar`. We keep head/chest/hands/legs/feet, melee, the two ranged slots
  and **one** item slot (the game plays a 4-slot loadout); rings and avatars are not modelled.
- `system/zshop.xml` (**observed**): 3 976 `SHOP_ITEM`, but every `ITEM_ID` (1, 2, 3, ...) is absent from
  `zitem.xml` (0 of 3 976 match): a legacy table, **unused**. Its comments define `ITEM_FILTER` 1 melee,
  2 ranged, 3 head, 4 chest, 5 hands, 6 legs, 7 feet, 8 accessory, 9 consumable, 10 avatar and
  `SELL_GROUP` 1 bounty, 2 cash, 3 medal shop.
- `system/gshop.xml` (**observed**, the live shop): 3 010 `SHOP_ITEM ID ITEM_ID ITEM_TYPE ITEM_FILTER
  RESALE SELL_GROUP EXPIRATION_DATE PRICE VISIBLE BEFOREPRICE [INFO]`, 2 971 `ITEM_ID`s exist in
  `zitem.xml`. `SELL_GROUP` 1 (2 087), 2 (663), 5 (256), 7 (4); `EXPIRATION_DATE` 0 (permanent, 1 027) or
  3 / 7 / 30 days (661 each); each item has one permanent offer. We list `SELL_GROUP=1` (bounty),
  `EXPIRATION_DATE=0`, `VISIBLE` true offers whose item has a name and (weapons) a model in
  `weapon.xml`: those are the items the game can actually use.
- `system/zitem.xml`: `sell_bt_price` is the sell price (1 166 items; 0 or absent on profile icons).
  `bt_price` is 0 for all but 6 items and unused. `res_level` gates buying, `res_sex` m/f/a gates
  armour, `hp`/`ap` of `equip` items are the armour bonuses (e.g. chest 3010000 `ap=30`).
  **Observed**: of the 739 permanent offers with a `zitem.xml` item, 421 have `PRICE = 10 x sell_bt_price`
  (e.g. katana 2010000: 16 200 / 1 620), 301 are profile icons with sell price 0 and 17 weapon/armour
  offers differ from that ratio.
- `interface/default/itemicon.xml` (**observed**, 3 050 `ITEMICONS`): `ICONID` `S<itemid>` (100x100) or
  `L<itemid>` (200x100), `SOURCE` atlas PNG in `interface/loadable/` (`itemicon_weapon_s_00.png`, `itemicon_male_s_00.png`,
  ...), `OFFSET` cell number counted row by row in a `FILESIZE` atlas of `BOUNDS` cells
  (cell `(OFFSET % (W/w), OFFSET / (W/w))`; **inferred** layout, verified by the shop shots). Missing
  icons fall back to `slot_icon_unknown.tga`.
- `system/grank.xml` (**observed**): 35 `RANK ID=1..35 RANK_NAME (Korean) ADR_NAME COLOR IMAGE` (`NEW`,
  `9K`..`1K`, `1D`..`5D`, `SNG`, `FRG`, ..., `GZM`); **no level or XP numbers**. The UI shows `ADR_NAME`
  (the retail font is not in the archives, Korean names cannot be drawn). The `COLOR` is unused.
- XP/bounty values elsewhere (**observed**): `mission.xml` daily kill missions pay `EXP 100` /
  `BOUNTY 100` for 3-20 kills; `scenario.xml` quest scenarios carry `XP`/`BP` (QL0 45/17 ... QL5
  2880/600, bosses up to 6000 XP); `map.xml` `ExpRatio` is 1 on all 37 maps (so no map multiplier);
  `hppercentformula.xml` lists HP-percent XP/bounty bonuses (50-90 %) whose rule is unknown, unused.

### Rules (all **inferred**)

- Levels 1..99 (`MAX_LEVEL`); going from level L to L+1 costs `100 x L` XP. Rank code =
  `ADR_NAME` of rank `1 + (level-1) x 34 / 98`.
- Kill: +10 XP, +10 bounty (scaled from the daily missions). Not in training. Match end: VICTORY +50/+50,
  DRAW +25/+25, DEFEAT +10/+10. A `game::Reward { xp, bounty, medals }` message pays anything else (quest clears,
  Blitzkrieg medals).
- New profile: 20 000 bounty (`gshop.xml` weapons cost 8 100-81 000), the default katana/revolver/rifle
  owned and equipped. Buying needs the level and bounty and costs the permanent `PRICE`; selling pays
  `sell_bt_price` and needs the item unequipped; melee and the two ranged slots cannot be empty.
  Equipping does not check the level (the starter rifle needs level 5).
- Equipped armour adds its `hp`/`ap` to the player's maximum health/armour at spawn. Armour has no
  model in the game (outfits are the character's `AddParts` sets, picked on the Player page).

### Profile file

`key=value` lines (`#` comments), saved under `$XDG_DATA_HOME/gunzrs/profile.txt` (else
`~/.local/share/...`; `%APPDATA%\gunzrs\profile.txt` on Windows), `GUNZ_PROFILE=PATH` overrides it.
Keys: `name` (default `$USER`), `woman`, `outfit` (`none` or a part-set index), `xp` (total, the level is
derived), `bounty`, `owned` (zitem ids), `equipped` (9 ids: melee, primary, secondary, item, head,
chest, hands, legs, feet; 0 = empty), `quest_items` (`zquestitem.xml` `id:count,..`), `medals` (Blitzkrieg
medals earned, shown on the profile header), `rented` (`zitem id:expiry,..`, expiry in Unix seconds; the id is
also in `owned`), `clan` (see "Clans"). A corrupt file stops the game instead of being overwritten.
Rentals past their expiry are removed at load (also from the equipment, melee and ranged fall back to the
starter weapons) with a log line `profile: rental of item N expired and was removed`; the inventory shows the time
left (`[rented, 2d 5h left]`). A rental cannot be sold (**inferred**).
Headless `--shot` runs use a throwaway default profile unless `GUNZ_PROFILE` is set. The file is
rewritten whenever the profile changes (a kill, a purchase, Start).
Headless shot hooks: `GUNZ_INV_SLOT=N` (9 = quest items) and `GUNZ_INV_SELL=1` (presses SELL on the first row).

## Clans (`src/clan.rs`)

The retail server owns clans; offline the profile holds one, and a "clan war" plays it against a generated
rival. **Observed** = read from the named retail file, **inferred** = ours.

### Data files

- `system/claniconinfo.xml` (**observed**): 120 `<CLANICONINFO><ICONID><SOURCE><OFFSET><EMBLEM><NAME><VISIBLE>`.
  100 emblems (`ICONID C1000000`..`C1000099`, `SOURCE ClanIcon_00.png`, `EMBLEM TRUE`) and 20 backgrounds
  (`C2000000`..`C2000019`, `ClanBG_00.png`, `EMBLEM FALSE`); `VISIBLE` is TRUE for 52 emblems and 9 backgrounds,
  the rest are empty atlas cells. `NAME` is `STR:CLAN_ICON_n` / `STR:CLAN_BG_n` of `strings.xml`. The 52 emblems are
  13 designs in 4 colours (REX, FLEX, VICS, MIZ, NICO, RIONIX, Urike, Renaut, ARES, L#, WALCOM, CANOX, MAXWELL; brown,
  gold, white, black); the backgrounds are Velvet (brown, gray, purple), Brushed Steel (silver, blue, red) and Slate
  (white, brown, green).
- Pictures (**observed**): `interface/loadable/clanicon_00.png` and `clanbg_00.png`, 1024 x 1024 RGBA, the pictures
  on a grid of 10 columns, `OFFSET` = row x 10 + column (so brown 0-12, gold 20-32, white 40-52, black 60-72; the
  100 px cell is **inferred** from the pictures, no file states it). `interface/default/clanicon_00.png` is a 114 px
  stub with one logo and is not used.
- `strings.xml` / `messages.xml` / `cserror.xml` (**observed**, English): clan page `UI_SOCIAL_CLAN_TAB_01..12`
  ("CLAN", "Create Clan", "Clan Leader", "Clan Officer", "Clan Member", "Clan Chat", "Clan Info", "Win/Lose :",
  "Point :", "Total Point :", "Ranking :", "Clan War Invitation"); `CLAN_MARK_EDIT_01..03` ("Emblem", "Background",
  "only once per minute"); `CLANWAR_UI_01..05` ("MATCHMAKING QUEUE", "Form Team", "Clan War", "Action Required",
  "Leave Team"); messages 1105-1127 (create, leave, kick, rank change prompts), 1301-1303, 1510-1515 (win-streak
  announcements) and 8007-8012; `cserror.xml` 30011-30054 (the refusals: name in use, not enough members, not
  the leader, level 10, bounty, emblem change). The port shows those texts verbatim where it needs one.
- `interface/default/clan.xml` (**observed**): the create dialog's name box `ClanCreate_ClanName` has
  `MAXLENGTH` 12. The comment on its text (Korean) says the dialog needs level 10, 1000 BP, a unique name of up to 12
  English letters (6 Korean) and 4 more founding members. The shipped text `UI_CLAN_12` says "level 10 or higher
  and 20,000 BT" while `cserror.xml` 30050 still says 1000 BT. The port takes level 10 and 20,000 BT.
- `interface/default/clanwar.xml` (**observed**): the war lobby, four `ClanWar_UserPannel_0..3` (level, win `승`,
  loss `패`, `KD`), region select, "balanced matching" check box, arranged-team dialog, matchmaking queue. All of it
  is server matchmaking; the four panels are the 4 against 4.
- `system/gametypecfg.xml` id 22 `GAMETYPE_CLAN_SCRIM` (**observed**): `ROUNDS` 3 (the only choice), `LIMITTIME` -1
  (unlimited, the only choice), `MAXPLAYERS` 8 (the only choice). `channelrule.xml` lists it only in rule 5
  `champion`, with the deathmatch maps (24 names, Mansion to Shower Room; any map works offline).
- `tips.xml` (**observed**, a Korean tip that is commented out): "in clan wars the EXP gain is 1.5 times and there is
  no EXP loss from level differences".
- Rating: `leaguekfactorsetting.xml` (K 50 from 0 games, 30 from 11, 20 from 51), `league.xml` `rating_gap` 300 and
  `leaguetier.xml` (25 tiers, 0..2400 in steps of 100) are **observed** for the ranked league; reusing them for clans
  is **inferred**.
- `interface/default/combat/ef_clan_win|lose|draw.elu` (**observed**) are the war's end banners; not used.

### Rules

- **Profile**: `clan=NAME|EMBLEM|BG|POINTS|WINS|LOSSES|a,b,c` in `profile.txt` (absent = no clan): the name, the
  `ICONID` numbers of the emblem and background, clan points, wins, losses and the bot members' handles. The player is
  the Clan Leader, the first bot the Clan Officer, the others Clan Members (ranks **observed** strings, assignment
  **inferred**). Name: 2..12 of letters, digits, space, `-`, `_` (the 12 is **observed**, the rest **inferred**).
- **Create** (menu CLAN tab): level 10 and 20,000 BT (**observed**, see above), a name no rival uses (`cserror` 30032)
  and 4 founding members; offline bots found the clan (12 handles that no rival wears, **inferred**). The tab also
  renames (`RENAME`), picks the emblem and background from the 52 + 9 visible ones (stepping them edits the clan
  at once), recruits and kicks bots (at most 11 **inferred**, at least 3 for a war) and leaves. `messages.xml` 1117 says
  the leader cannot leave a clan (only disband it, 1108); the player is always the leader, so `LEAVE` (asks twice,
  1123) deletes the clan and its points. The name box takes typing once clicked (Enter ends it).
- **Rivals** (**inferred**): one per retail emblem design, 13 in all, named after it (REX 700 points, FLEX 750, ...
  MAXWELL 1300), wearing that design in colour `index mod 4` on background `index mod 9`, four handles each. They
  are static. The ranking table lists them with the player's clan (`Ranking : n / 14`).
- **Match-up** (**inferred**): of the rivals within the league's `rating_gap` 300 of the clan's points, the one
  `games played mod count` picks (so wars rotate); none within the gap: the nearest.
- **Clan war** (`--mode clanwar`, the menu's "Clan War"): game type 22. 4 against 4 (`MAXPLAYERS` 8): the player and the
  first 3 bot members (Red) against the rival's 4 (Blue), 7 bots whatever `--bots` says. The rules are Elimination's
  (`Mode::rounds` + `teams`): no respawn until the round ends; the kill limit counts round wins, default 3 (`ROUNDS`),
  no time limit. A war needs a clan: `gunz-play --mode clanwar` without one exits with a message, the menu's
  Start opens the CLAN tab instead.
- **Presentation**: actors are named `Clan.Handle` (scoreboard and everything that prints names); the HUD header has
  each clan's emblem and name beside the clock; the kill feed is redrawn with both clans' emblems (the stock text
  feed is silent in a war); the scoreboard (Tab, match end) gets a strip with both clans' emblems, names and points,
  and the player's clan's gain once the war is settled.
- **Settlement** (`clan::settle`, once, when the match is over and `profile::finish` paid its result): Elo with the K
  factor above, expected score `1 / (1 + 10^((rival - mine) / 400))` (400 **inferred**), `delta = round(K x (score -
  expected))`, score 1 / 0.5 / 0 for VICTORY / DRAW / DEFEAT (a draw needs a time limit; the war has none by default),
  points never below 0, wins and losses counted. The XP bonus is **observed** (the 1.5x tip): the match's XP (kills
  and result) gets half again through `game::Reward`; bounty is not mentioned and is paid as in any match. One log
  line, e.g. `gunz::clan: clan war VICTORY: Phoenix 1000 -> 1025 points (+25) against REX (700) ...`.

### Not modelled

Everything that needs other people: clan chat, invitations and joining (`messages` 1105-1114), delegating the leader,
changing member grades, the war lobby, matchmaking queue and regions, win-streak announcements (1510-1515), the
one-per-minute mark edit, the 48 hour disband delay, "Total Point" (no seasons, so it would equal "Point"), the end
banners `ef_clan_*`. Rival clans do not play each other, so their points never move.

## Install discovery and platforms (`src/steam.rs`, `src/bin/gunz-play.rs`)

No game file format; **observed** = read from a named Steam file on the development machine.

- **Observed** (`steamapps/appmanifest_3139440.acf`, Flatpak Steam): `"appid" "3139440"`,
  `"installdir" "GUNZ THE DUEL"`; the game is `<library>/steamapps/common/GUNZ THE DUEL`.
  `steamapps/libraryfolders.vdf` lists the libraries as `"path" "..."` lines (backslashes doubled on
  Windows). `gunz-play` without GAME_DIR (first argument is not a directory) checks, in order, the Steam
  roots `C:\Program Files (x86)\Steam`, `C:\Program Files\Steam`, `~/.steam/steam`,
  `~/.local/share/Steam`, `~/.var/app/com.valvesoftware.Steam/.local/share/Steam` (Flatpak) and
  `~/Library/Application Support/Steam` (macOS), and for each the root and every `path` library.
  The Windows and macOS root locations are **inferred** (Steam's defaults, not checked here).
- Restarting into a match or back to the menu (`relaunch`) `exec`s on Unix; elsewhere it spawns
  the new process and exits (**inferred** to be equivalent: the old window is gone either way).
- `Vfs::mount` keys archives by lowercased, `/`-normalised relative paths, so Windows `\` separators
  and case-insensitive file systems need no special handling.

### Effect warm-up (`effect::WarmFxPlugin`)

First use of an effect parses its ELU/ANI, decodes its textures, creates materials and makes the
renderer compile pipelines on the frame it is needed. Once the camera exists, the plugin spawns the 18
effects the match code uses (`WARM` in `src/effect.rs`: muzzle flashes, sword hit and flash, explosions,
smoke trail, heal/repair auras) at 1/1000 scale a metre in front of it (**inferred** scale and
distance: small enough to be invisible, inside the frustum so they are drawn).

## Quest (`src/quest.rs`)

Offline `gunz-play --mode quest --scenario NAME [--dice N] [--sacrifice A,B]` (no MAP: the first sector is the map).
Labels as above: **observed** = read from the named retail file, **inferred** = ours (the server-side NPC-set
file and all rules are not in the data).

### Data files

- `system/scenario.xml` (**observed**): 12 `<STANDARD_SCENARIO QL title DC mapset XP BP>` (Mansion and Prison,
  QL 0-5; XP 45..2880 / 90..5760, BP 17..600) and 8 `<SPECIAL_SCENARIO id title QL ... >` (Mansion 11 Goblin
  King, 12 Fake Goblin King, 13 Thunder Goblin King, 14 Dwarf Goblin King, 41 Captain Pampow; Prison 21 Lizard
  King, 22 Golem, 42 Palmpow) with two `<SACRI_ITEM itemid>` (the offering a special quest costs). Every
  scenario has 6 `<MAP dice key_sector [key_npc boss]>` (`dice` 1..6 in all 20 scenarios) with
  `<NPCSET_ARRAY>G11/G12/..</NPCSET_ARRAY>`; the
  four `JACO` bosses (11, 12, 21, 22) add `<JACO count tick min_npc max_npc>` with `<NPC npcid rate>`
  reinforcements. There are no Dungeon scenarios. `DC` (1 everywhere) has no explained meaning.
- `system/sacrificetable.xml` (**observed**): 10 `<ITEM map ql default_item_id special_item_id1/2
  significant_npc sdc ScenarioID>` rows. `default_item_id` is 0 at level 1 and 200001..200004 (the Torn Pages
  I-IV) at levels 2..5; the other rows give special items and the boss they draw (`200008` "goblin chief",
  `200018` "goblin king", `200022` "palmpow", `200024` "palmpoa commander", `200025`+`200027` "cursed palmow").
  `map` and `ScenarioID` are empty/0 in every row, so the table does not name the scenarios: the pairs of
  `scenario.xml` `SACRI_ITEM`s do (they are not the same pairs as the table's rows).
- `system/questmap.xml` (**observed**): 3 `<MAPSET>` (Mansion, Prison, Dungeon) of 9 `<SECTOR id title
  melee_spawn range_spawn>` (`melee_spawn=range_spawn=15` in every sector of both files), each with `<LINK
  name><TARGET sector=title/>..`.
  `title` lower-cased is the directory under `quest/maps/` (27 of 27 resolve, `map::find_rs`); `LINK name` is
  the portal dummy `linkNN` of that map. `quest/maps/*/spawn.xml` (all 27 checked) is `<GAMETYPE id="solo"/>`
  and `<GAMETYPE id="team"/>` with no children: there is nothing in it to use.
- Quest map dummies (**observed**): `spawn_solo_101..104` (the player's), `link01..`, `spawn_npc_melee_NN`,
  `spawn_npc_range_NN`, `spawn_npc_boss_NN`, `wait_pos_01` (one per map). Mansion_Hall1: 4 solo,
  12 melee, 10 range, 1 boss, 1 link. `Level::spawn_points` leaves out `spawn_npc_*`.
- `quest/maps/*/*.rs.nav` (**observed**, all 27 maps, the layout accounts for every byte): `u32` magic
  `0x8888888f`, `u32` version 2, `u32 nv`, `nv` x (x y z `f32`, map cm), `u32 nt`, `nt` x 3 `u16` vertex ids,
  `nt` x 3 `i32` neighbour triangle across each edge (-1 = border): a walkable triangle mesh (12..507
  triangles, 222..2 439 m2 per map). Not used, see "Not supported".
- `system/scenario2.xml` (**observed**, the challenge quest, `GAMETYPE_QUEST_CHALLENGE` id 12): 8
  `<SCENARIO map_id name reward_item players level_limit good_time_sec>` (101/201/301/401 for 4 players,
  102/202/302/402 for 3; `level_limit` 1, 21, 41, 61 / 11, 31, 51, 71; `good_time_sec` 480, 960 for 401, 720
  for 402; the file's comment calls it the recommended clear time), 6 `<SECTOR map xp bp>` each with `<SPAWN postag
  num actor drop [adjustplayernum]>`:
  `num` NPCs `actor` (27 distinct `npc2.xml` `<ACTOR name>`s) at the dummies `spawn_npc_<postag>` of
  `challengequest/maps/<map>/` (the name repeats for many dummies; `boss` = `spawn_npc_boss`), `drop` is
  `C1`, `C2` or empty. Maps repeat inside a scenario (`R_Normal` x5).
- `system/survivalmap.xml` (**observed**, `GAMETYPE_SURVIVAL` id 6): the `questmap.xml` schema, 3 map sets of
  5 sectors whose first `LINK` leads to the next one, closing a loop (Mansion 109 -> 108 -> 105 -> 103 ->
  102 -> 109). No NPC data of its own.
- `system/droptable.xml` (**observed**): 32 `<DROPSET id name>` (24 distinct names; `G181` is listed several
  times) of `<ITEMSET QL=0..5>` with `<ITEM id rate>`. In all 147 item sets the rates add up to at most 1
  (several reach exactly 1), so a roll walks the cumulative rates and may drop nothing. `id`: `hp1`/`ap1`/`mag1`
  (a world item), 2000NN (a `zquestitem.xml` item), 2xxxxxx / 3xxxxxx (shop items of `zitem.xml`; a few with
  `rent_period`). `npc.xml` `<DROP table>` names them (`G11`..`G19` for NPC ids 11..19, `K21`.., `S31`..,
  `P41`..).
- `system/zquestitem.xml` (**observed**): 45 `<ITEM id=200001..210001 name=STR:QITEM_NAME_<id> type level
  unique price secrifice param grade>`, type `page skull fresh ring necklace doll book object sword monbible`.
  `secrifice="1"` marks what may be sacrificed (all but the `fresh` ore/scrap/emblem and the monster bible);
  `level` is 5/10/15/20 on the four pages and 0 otherwise. Names: `QITEM_NAME_<id>` in `strings.xml`; in 20 cases
  (19 items and 210001, which has no string) they are Korean or missing in `strings.xml` and in all ten locale
  directories (`chn deu esp fra jpn kor pol prt rus spn twn` carry the same Korean), so `quest::KOREAN_NAMES` has
  **inferred** English translations (200005 Small Skull, 200006 Large Skull, 200007 Mysterious Skull, 200010
  Giant Remains, 200019 Skeleton Doll, 200023 Rabbit Doll, 200024 Teddy Bear, 200025 Cursed Teddy Bear, 200028
  Devil's Dictionary, 200029/30 Scryder's Roster Part 1/2, 200031 Blessed Cross, 200032 Cursed Cross, 200034
  Talking Pebble, 200035 Ice Crystal, 200040 Superion's Sword, 200041 Aneramon's Sword, 200042 Lich's Tail, 200043
  Pampow's Ice Sword, 210001 Monster Bible). The 25 others use the retail English names. Icons: `itemicon.xml`
  has `S2000NN` entries (atlas `itemicon_Quest_s_00.png`) for a few.
- `system/npc.xml` (**observed**): `<NPC id grade offensetype>`; ids 11..19 goblins, 21..26 kobolds/golem,
  31..39 skeletons (no scenario or `droptable.xml` set uses them), 41..48 palmpoas, 15x / 16x / 17x copies with a
  third of the HP (used below for QL 0).

### Rules (**inferred** unless noted)

- Plan: a standard/special quest starts at the first `SECTOR` of its map set and walks the shortest `LINK`
  route (breadth first over titles) to `key_sector`; the `<MAP>` is the **dice roll**: `--dice N` picks the
  `dice` N, otherwise one is rolled uniformly over the scenario's maps (seed: the clock, or `GUNZ_SEED=N`;
  headless `--shot` runs use seed 1, so they repeat). The roll is logged (`quest: dice roll 4 of 6: ...`) and on the
  HUD (`DICE 4` in the sector line). `--dice` survives "Play again" (`Config.dice`); a rolled one is
  rolled again on every start. The last sector holds `key_npc` (specials) at `spawn_npc_boss_01`; clearing it
  ends the quest. A challenge quest chains its `SECTOR`s with `link01`. Survival plays 10 sectors of the loop
  with the NPC sets of standard quest levels 1, 1, 2, 2, ... 5. **Survival Dungeon** (re-checked: no scenario,
  `questmap.xml` quest, `droptable.xml` set or `npc.xml` entry names a Dungeon NPC set, but `survivalmap.xml`
  does have a Dungeon loop of 5 sectors and the skeleton family 31..36 is the one family no scenario uses) is
  offered with **inferred** sets `S<ql>1..` = skeletons 31..34 (35 from level 2, 36 the Lich from level 4), the
  XP/BP of the first map set's standard quest of that level / 4; skeleton drop tables `S31`.. do not exist, so
  the challenge fallback below applies. `Catalog::names()` lists the 31 names: the 20 scenario titles,
  `Challenge <map_id>`, `Survival Mansion|Prison|Dungeon`; a bare scenario id / `map_id` also selects.
- NPC sets: the sets are not in the data. `Xqn` (family letter `G`/`K`/`S`/`P`, level digit, member) is NPC id
  `10/20/30/40 + n` (`G14` -> 14); at level 0 the weak copy `150/160/170 + n`. `grade="boss"` NPCs never come
  from a set (the 5th member of the kobold sets would be the Lizard King). Sector size `8 + 2 x QL` NPCs
  drawn uniformly from the sets (half as many next to a boss), `offensetype="2"` NPCs (gunners, wizards) at the
  `spawn_npc_range_*` dummies, the rest at `spawn_npc_melee_*`; HP/AP `x (1 + 0.25 (QL-1))`. Per sector at
  most `melee_spawn` melee and `range_spawn` ranged NPCs live at once (the attributes' meaning is **inferred**;
  15 each, so it only binds on the biggest sectors; challenge maps have none), the queue spawns one every 0.4 s
  after a 3 s "SECTOR n" intro. `JACO`: while the boss lives and
  fewer than `max_npc` NPCs are alive, `count` NPCs picked by `rate` appear every `tick` s at melee dummies.
  `adjustplayernum` bosses get HP `x (player + bots) / players`.
- Sacrifice: the two sacrifice slots (`--sacrifice A,B`, or the menu's "Sacrifice 1/2" steppers over the
  profile's `secrifice="1"` quest items) pick the scenario. **Observed**: a pair that is a special scenario's
  two `SACRI_ITEM`s (either order) is that scenario (Goblin King = Goblin Skull 200008 + Grimsk's Necklace
  200018); standard quests of level 0 and 1 have no `default_item_id`. **Inferred**: a standard quest of level 2..5
  needs the Torn Page of the table row of its level in a slot; a page needs the character level in its `level`
  (5/10/15/20); a challenge quest needs `level_limit` as the character level; the start spends one of each
  needed item (also on "Play again", so a replay of a special quest needs new items and the menu opens with the
  reason when they are gone). The menu's line under the steppers says "ready", what is missing or the level
  needed, and `sacrificetable.xml`'s `significant_npc` as a hint ("Goblin Skull draws a goblin chief"); Start
  does nothing while it is not ready. Items come from drops; the pages 200001..200003 drop nowhere and no shop
  sells any, so edit `quest_items=` in the profile to try them.
- Waiting room (**inferred** from the name `wait_pos_01`, the one such dummy of a quest map, standing on a gallery
  above the hall, e.g. Mansion_Hall1 (-525, -1495, 405) against the `spawn_solo`s at (1116, -163..159, 225)): in
  the 3 s "SECTOR n" intro the player waits there, facing the dummy's direction, and then drops to the first
  `spawn_solo` as the NPCs start to come (not with `--at`/`--yaw`). The log lines `quest: <npc> dropped <item> at
  (x, z) t=<s>` and `sector n cleared at m:ss` carry the quest time.
- Headless checks of the quest screens: `--mode quest --scenario NAME --sacrifice A,B --menu-page match` shows
  the picker (a `--menu-page` never starts a quest); `GUNZ_INV_SLOT=9 --menu-page inventory` opens the
  inventory on the quest-item category.
- Clear: no NPC alive and none queued. The `linkNN` portal opens (a cyan cylinder); walking within 1.2 m
  (or 30 s later) swaps the map in-process (despawn map entities, NPCs, drops and bots; reload `Level`,
  `MapCollision`, props, spawn table, `PostStartup`: bots with a new `Nav`), the player (keeping HP/AP/ammo)
  stands on the first `spawn_solo`. Dead actors do not respawn; the quest fails 2.5 s after the player dies.
- Rewards: `Reward{xp, bounty}` = the scenario `XP`/`BP` when the last sector falls; challenge sectors pay their
  `xp`/`bp` on each clear (survival: the standard quest's of that level / 4); a cleared challenge adds its
  `reward_item` to the loot. A challenge cleared within `good_time_sec` (HUD `TIME m:ss/m:ss`) pays a further
  25% of the XP/BP its sectors paid (**inferred**: the data gives only the recommended time).
  `QuestLoot{items, rented}` carries every quest/shop item picked up, once at the end; the profile keeps the quest
  items (`zquestitem.xml` ids, `quest_items=id:count,..` in `profile.txt`; the inventory page has a "Quest items"
  category with names, counts, descriptions and a SELL button) and the rentals (`rented`, below); the challenge
  `reward_item` 3000xxx is a gacha package id that is not in `zitem.xml`.
- Rental drops (`droptable.xml` `rent_period`, 3 items x 72 / 168 in the data): **observed** unit is **hours**:
  the values are 3 and 7 days, `eventshopitem.xml` / `mission.xml` / `gunzplus.xml` name the same kind of
  number `rent_hour_period` / `renthourperiod` / `*_rent_hour_period` (720 = 30 days), and message 11002 counts
  "day(s) hour(s)". A pickup with `rent_period` rents the item for that many hours from the wall clock
  (`std::time::SystemTime`); a rental of an item the profile owns for good changes nothing, one of an item already
  rented keeps the later expiry.
- Selling quest items (inventory page, SELL): **inferred** the `zquestitem.xml` `price` is the bounty paid per
  item (no shop price exists for it, so the shop's `PRICE = 10 x sell_bt_price` ratio has nothing to start from;
  the port pays `price` as it pays `sell_bt_price`). One item per click; log `shop: sold NAME (ID) for N bounty`.

### Not supported

- `.nav` triangle meshes: decoded above and measured against the dummies, but a mesh holds only 843 of 914
  (92%) spawn/link dummies (Dungeon_Cavern3 and Nest2 about 57%), so `nav.rs`'s floor graph, which covers every
  map, has to stay as the fallback; one source is simpler, so the quest NPCs keep using it.
- The quest `spawn.xml`: empty stubs. Scenario `DC`, `sdc` (sacrificetable) and per-NPC `dc`: no meaning in
  the data. The gacha `reward_item` (3000xxx) of the challenge quest and the quest-item shop (no shop in the
  data sells quest items). Permanent shop-item drops (`droptable.xml` items of 2xxxxxx / 3xxxxxx without
  `rent_period`, rate 0.001): dropped, not added to the inventory. Online party/lobby behaviour.

## Quest monsters (`src/npc.rs`, `src/npc/data.rs`, `src/npc/fsm.rs`)

Labels: **observed** = read from the named file, **inferred** = our reading (the executable is packed).
`quest.rs` sends `SpawnNpc{id, pos, yaw, hp_scale, drop, boss, team, route}` (`team`: `None` = from the name, `route`: a
`game::Routes` id, 0 = none; Blitzkrieg uses both); `id` is an `npc.xml` id (`"16"`) or an `npc2.xml`
actor name (`"knifeman"`). The monster is an entity with `Npc`, `Vitals` (`max_hp`/`max_ap` x `hp_scale`),
`Team::Blue` (`_red` actors: Red), `HitShape`, a skinned model and a `Brain`; combat, melee, blasts and bots treat
it like any actor. `gunz-play MAP --npc NAME[,NAME..]` spawns some `--bots-ahead M` metres in front of the player
(`GUNZ_NPC_HOLD=S` keeps them idle for S seconds), e.g. `--npc 11,31,22` or `--npc knifeman,tower`.

### `system/npc.xml` (**observed**: 76 `NPC`, ids 11-19, 21-26, 31-39, 41-48 and the quest-level variants 111-191, 2011-2024)

`<AI_VALUE>`: `SHAKING pathfinding_update="0.1" attack_update="0.1" speed="0.2"`, `INTELLIGENCE` and `AGILITY` with
five `<TIME step="1..5">` seconds (0.4 0.6 1 2 3 and 0.2 0.5 1 2 3). `<NPC id name="STR:NPC_NAME_n" desc meshname
scale="x y z" grade max_hp max_ap int agility view_angle dc offensetype dyingtime>` with children `COLLISION radius
height [tremble pick]` (cm, absolute: the Lich has `scale 0.17` and radius 120), `FLAG never_pushed never_blasted`,
`ATTACK type="melee" range weaponitem_id [hitrate]` (cm; the item is a `30001x` melee weapon of `zitem.xml` that
has `damage`, `range`, `angle`, no model), `SPEED default [rotate]` (cm/s, rad/s), `SKILL id` (110 uses of 48 skills),
`DROP table` (`droptable.xml` set name). Grades: boss 26, regular 23, elite 15, veteran 12; `offensetype` 1 melee 61,
2 caster/gunner 15; `dyingtime` 0, 5 or 8 s. `name` resolves through `strings.xml` `NPC_NAME_n`; 22 of the 34 distinct
names (47 NPCs) exist only in Korean in every locale: see "Closed gaps" below for the English table.
Use (**inferred**): `int` indexes the `INTELLIGENCE` table, `agility` the `AGILITY` table = seconds between two melee
blows; `view_angle` is the facing tolerance before a blow (>= 25 deg); `offensetype 2` casters stop 7 m away when they
own a missile skill; `dc` and `tremble` are not used. A melee blow lands at 45 % of the `melee_attack` clip with the
weapon item's damage, `ATTACK range` and the item's swing angle; palmpoas (no weapon) hit for 10.

### `system/npc2.xml` (**observed**: 48 `ACTOR`, 11 `boss`)

`<ACTOR name model ai.fsm max_hp max_ap collision.radius collision.height speed rotspeed groggyRecoverPerSec
neverblasted [boss meshpicking grenadecollision forced.collup120] sound.die>`: guerrillas (`knifeman` `hunter`
`rifleman` x3 tiers, bosses `robot` `psychic` `general`), the research-lab bots (`charger` `shooter` `disposer` x3,
`chaser` `tower` `assassin` and their `ex` variants), and the Blitzkrieg set (`b_*_red/blue`, `radar`, `barricade`,
`blitzbox`). `speed` cm/s, `rotspeed` rad/s; `groggyRecoverPerSec` is the groggy decay per second (**inferred**).

### `model/npc.xml`, `model/npc2.xml`, `model/npc/<dir>/<name>.xml` (**observed**)

Registries `AddXml name filename` (70 models: 25 + 45) with the character-XML layout of `man01.xml`: one
`AddBaseModel`, `AddAnimation name filename motion_type="0" motion_loop_type`. The models carry weapons in the mesh
and have no `AddParts`. Clip names, classic: `idle neglect1/2 melee_attacked1/2 range_attacked1 lightning
melee_attack run die [die2] special_attack1..4 stunned` (+ the blast set); actors: `idle run run2/3 suffer1/2/3
suffer3recover step* charge slash* die` and each boss's own. One file is missing from the archives
(`goblinG` `range_attacked2`): that clip is skipped. Every other clip and mesh of the 70 models exists.
`system/animationevent.xml` (see Animation) gives the sounds per classic clip; they play as `PlaySound`.

### `system/zskill.xml` (**observed**: 49 `SKILL`, namespace `zskill.xsd`)

`id name resisttype hitcheck guidable velocity delay lifetime colradius difficulty knockback effecttype
effectstarttime effecttime effectarea effectareamin effectangle effect_startpos_type mod.damage mod.dot
mod.criticalrate mod.speed mod.antimotion mod.root mod.heal castinganimation castingeffect castingpreeffect
castingeffectAddPos traileffect traileffecttype traileffectscale sound.explosion camera.*` and `<REPEAT delay
angle="x y z">` children. By (`hitcheck`, `effecttype`): (true, 0) 18 and (true, 1) 6 and 1 more are missiles,
(false, 4) 12 area hits, (false, 2) 5 ground discs (blizzard), (false, 6) 4 heals, (false, 0) 3 slow/stun.
Use (**inferred**): `castinganimation N` plays `special_attack<N>` (none: `melee_attack`); `effectstarttime` ms
is when the effect happens inside the clip; `delay` ms is the reuse time; missiles fly `velocity` cm/s with a
`colradius` cm sphere, `lifetime` ms (0: 6 s) and home on the target when `guidable`; each `REPEAT` fires another
missile `delay` s after the previous one, turned about Y by `angle.z` rad; `effecttype 4` hits everything between
`effectareamin` and `effectarea` metres inside a cone `effectangle` deg wide (>= 360: all round) - the Goblin King's
Massive Swing has the band 5.8-10.2 m and `castingeffectAddPos="780 0 0"`, an effect 7.8 m ahead (x runs forward);
`effecttype 2` is a disc of `effectarea` m at the target; `mod.damage` is the damage; `knockback` is cm/s of
horizontal push (like `zeffect.xml`); `traileffect` is an `effect_list.xml` name spawned along the flight at 0.07 s
(x `traileffectscale` / 2). `mod.speed` < 100, `mod.root` and `mod.dot` become an `Afflict` on whatever the skill
hits ("Status effects" under Actors). Not supported: `mod.criticalrate`, `camera.*` shake, `resisttype`
resistances, `surfacemount`.

### `system/zactoraction.xml` (**observed**: 183 `ACTION`)

`<ACTION name animation [movinganimation]>` with children `EFFECT delay mesh posparts posmod dirmod scale` (345),
`SOUND delay sound` (250), `MELEESHOT delay damage range angle pierce sound [uppercut thrust]` (80), `RANGESHOT
delay damage pierce sound mesh speed collradius dirmod posparts dirtarget [zaxis yaxis thrust]` (225),
`GRENADESHOT delay damage pierce grenadetype itemid zaxis yaxis force posparts posmod dirmod [sound]` (62) and
`SUMMON name delay range angle [adjustplayernum drop route]` (126); `delay` ms from the action start.
Use (**inferred**): `movinganimation` = the clip's root bone moves the actor (the others walk at the state's
speed); `MELEESHOT range` cm from the actor, `angle` deg of the fan (at least 16 so a narrow `charge` still lands),
`uppercut` throws the victim up (Push y 8), `thrust` pushes it 6 m/s; `RANGESHOT` leaves from the bone `posparts`
(`lhand` `rhand` `head` = `Bip01 L/R Hand`, `Bip01 Head`) toward the target plus `dirmod` (x sideways, y up, z
forward), `speed` cm/s, `collradius` cm, effect `mesh` as the trail; `GRENADESHOT` lobs at `force` cm/s, `yaxis` deg
above the horizon, `zaxis` deg off the facing, blast 3.5 m, fuse 1.5 s, gravity 14 m/s^2 (`itemid` 40505/40506 is
not in `zitem.xml`); `grenadetype 3` bursts on first contact; `SUMMON` spawns actor `name` `range` cm away at
`angle` deg. `pierce` (per cent) is the share of the blow that reaches health: MELEESHOT 0, RANGESHOT 50 and
GRENADESHOT 0 are the `Damage.pierce` of what the actor does.

### `system/aifsm.xml` (**observed**: 38 `FSM`, 559 `STATE`, 2 857 `TRANS`)

`<FSM name entrystate><STATE name cooltime action func enterfunc exitfunc><TRANS cond next/></STATE></FSM>`;
every `npc2.xml` `ai.fsm` resolves, every `action` exists in `zactoraction.xml` (the Blitz `barricade_die` and
`radar_die` have `animation=""`, no clip). `cond` is the AND of comma terms, `next` a state or the built-in `__die`.
Conditions (17, with uses): `groggyGreater:N` 1 037, `hpEqual:0` 540, `dice:N` 408, `timeElapsedSinceEntered:ms` 374,
`endAction` 358, `distTarget:min;max` 271, `canSeeTarget` 143, `hasNoTarget` 98, `hasTarget` 72, `default` 72,
`FailedBuildWayPoints` 47, `isEmptySpace:angle;cm` 45, `angleTargetHeight:min;max` 32, `lookAtTarget:deg` 30,
`SummonLess:N` 10, `cannotSeeTarget` 4, `TargetHeightHigher:cm` 2. Functions (15): `findTarget`,
`findTargetInHeight:cm`, `findTargetInDist:cm`, `dice`, `rotateToTarget`, `faceToTarget`, `faceToLastestAttacker`,
`buildWaypointsToTarget`, `clearWaypoints`, `runWaypoints`, `runWaypointsAlongRoute`, `runAlongTargetOrbital:cm/s`,
`turnOrbitalDirection`, `speedAccel:cm/s^2`, `reduceGroggy:N` (`func` every step, `enterfunc`/`exitfunc` once).
The parser rejects anything outside this list. Semantics (**inferred**), executed by `npc.rs`:
- A step every 0.1 s (`SHAKING attack_update`): `func`s that choose the target, then the first `TRANS` whose terms
  all hold and whose target state is off cooldown (`cooltime` ms since that state was last entered); turning,
  running, orbiting and `speedAccel` act every frame.
- `dice:N` (replaces the first reading, "true with probability N/1000 per step", which let the Research Lab disposer
  shoot 3 times in 100 s): all 104 states with a `dice` row (29 `func="dice"`, 75 `enterfunc="dice"`, none without; all
  have 2 or more rows) hold one stored random number, rolled on entering (`enterfunc`) or every step (`func`). The
  state's `dice` values sum to 50..1 300 (18 states 100, 24 states 225 = 75+75+25+25+25, three above 1 000), so
  they are neither percent nor permille: each row owns the next `N` of the sum, in file order, and the roll picks
  the row (`waitrandom`: four equal waits; `orbit`: three equal orbit radii; the disposer's `orbit1`: "orbit" 2/3,
  "shoot" 1/3 once it sees the target, the 4 s timeout is only the fallback). A row whose other terms fail still
  keeps its share, so the roll finds nothing that step. Measured on the disposer: "Closed gaps" below.
- `groggy` rises by the damage taken and decays by `groggyRecoverPerSec`; `groggyGreater:20/30/40` pick
  `suffer1/2/3` and `reduceGroggy` clears it (`9999` = all). `hpEqual:0` is `Vitals.hp <= 0`; `__die` plays `die`
  and removes the corpse after 4 s (a dead actor stuck in a state with no death row is put down after 1.5 s).
- `endAction`: the state's action clip has ended (a looping clip never ends). `distTarget` is in cm.
  `isEmptySpace:a;d` asks whether the floor continues `d` cm in the direction `a` deg clockwise of the facing
  (0 front, 90 right, 180 back). `SummonLess:N`: fewer than N living summons of this actor.
- `speedAccel:N` sets the acceleration (cm/s^2; the speed target is `speed`, or the orbit speed), so `speedAccel:1`
  keeps the speed a state was entered with. `runAlongTargetOrbital:v` circles the target at v cm/s (`turnOrbital
  Direction` flips the sense), `runWaypoints` follows the `nav.rs` route (straight line when the floor is continuous),
  `runWaypointsAlongRoute` walks the spawn's route waypoint by waypoint (`nav.rs` between them) whether or not a
  target is known (Blitzkrieg's lanes; with no route it does nothing). Every monster is a possible target of every
  monster of another team (Blitzkrieg's two sides; the quest's monsters are all one team); the actor's entry state's
  action starts at once. `game::NpcState` forces a state by name (Blitzkrieg's radar reinforcements).

### Not supported

`pick` meshpicking and `tremble` (no use found: the shake of the boss skills is `camera.*`); the `.nav` files of the quest
maps (the floor graph of `nav.rs` is used); `resisttype` resistances and `surfacemount`.

### Closed gaps (NpcPolish; labels as above)

- **Names**: `NPC_NAME_n` of 22 monsters (21-26, 31-39, 41, 42, 44-48) is Korean in `strings.xml` and in all eleven
  locale directories (`chn deu esp fra jpn kor pol prt rus twn` carry the English strings of the other 12 and the same
  Korean for these; `spn` is all Korean); `interface/monsterillust/*.jpg` are pictures with no text. `data::ENGLISH_NAMES`
  holds **inferred** English names translated from the Korean (리쟈드 Lizard, 샤만 Shaman, 대장 Captain, 왕 King, 고장난
  골렘 Broken Golem, 스켈레톤 메이지 Skeleton Mage, 거대 Giant, 저주받은 시신 Cursed Corpse, 리치 폰 Lich Pawn, 슈페리온 Superion,
  아네라몬 Anelamon, 팜포우/팜포아 Pampow/Pampoa, 저주 받은 Cursed); `(보스)` is kept as "(Boss)". They fill in only where no
  Latin string exists, so the HUD, kill feed and `Missile` / `Damage` names show them.
- **`mod.criticalrate`** (0-90 per cent, **observed** in `zskill.xml`): chance of a critical hit on a missile's direct hit
  and on each target of an area skill; the multiplier `CRIT` 1.5 is **inferred** (not in the data). Seen: Goblin King
  Massive Swing (100 -> 150), his Fire Missile (80 -> 120), golem rocket (40 -> 60).
- **`camera.power/duration/range`** (**observed** on 15 skills: Massive Swing 3.0 / 1.5 s / 1500 cm, the stun fist 1.0 /
  0.7 s / 600 cm): an area skill sends `game::CameraShake { at, trauma, range }`; `hud::track` raises the same trauma
  shake as explosions, scaled to zero at `range`. `trauma = power * duration / 4.5` (clamped to 1) is **inferred**.
- **`SUMMON route`**: already parsed (`Ev::Summon.route`), copied into the `SpawnNpc` of the summoned soldier and walked by
  `runWaypointsAlongRoute` (Blitzkrieg's radar summons); nothing was missing.
- **Orb size**: the glow of a missile is scaled by `colradius` (golem rocket 261: 90 cm = a 1.8 m ball) but never wider than
  the trail effect (`traileffectscale / 2`, 3.5 for 261). `colradius` stays the hit test. **Inferred.**
- **`dice`**: see the `dice:N` paragraph under `aifsm.xml`. Disposer, Mansion, `--npc disposer --bots-ahead 10`, 60 s:
  14 `shoot` entries (was 3 in 100 s).
- **`lab_chaser_summon`** (actions `chaser_summon`, `chaser_summon2`) has no wav in `sound/effect/challenge_quest/researchlab`
  (there are `lab_chaser_{arrive,run,runfast,dash,launch,suffer1-3,die}`, `lab_tower_summon`, `lab_assasin_summon`):
  `npc.rs::play` plays `lab_tower_summon` instead (**inferred**, the same three-minion summon with a dust puff).
- **Goblin stuck on Mansion** (`gunz-play Mansion --bots 0 --bots-ahead 4 --npc 11`, player at the spawn on the y = 6 m
  floor): the goblin walked to 2 m from the player and stopped for good. `MapCollision::slide_move` returned no movement
  there although every ray (heights 0.2-1.5 m, both ways) and a downward ray line show open, continuous floor, and
  slides of the same length in the other seven directions move freely (a sweep-only snag, cause not found in `col.rs`).
  `npc.rs::locomote` now retries a blocked step (less than 30 % of the intended length) at +-30, 60 and 90 degrees and
  takes the first that makes half the headway (**inferred** remedy). Result: the goblin reaches 1.05 m and hits. A
  monster that falls to another floor now also notices a player within 12 m (`HEARING`, **inferred**) and keeps its
  target within 67 m without sight, so it follows the nav graph instead of idling; the demo spawn 8 m ahead lands
  over the gap in front of that spawn, so the goblin falls to the ground floor (a different, expected, effect).
