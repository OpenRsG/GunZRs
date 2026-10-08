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
- **Events**: none for player clips. `system/animationevent.xml` has only quest-NPC sound
  events (`<Animation name=…><AddAnimEvent eventtype="sound" filename beginframe>`); no
  player clip, effect or footstep event exists in the data (steps are inferred in `actor.rs`).
- **Cross-fade** (`Animator::play`): nothing in the data gives a blend time (**inferred**
  by the caller). The pose on screen is captured and smooth-stepped into the new clip.
- **Aim pitch** (`Animator::aim_pitch`, **inferred** split): applied after sampling and
  layering as a model-space rotation about the lateral axis, 50 % each on `Bip01 Spine1` and
  `Bip01 Spine2` so the legs (children of `Spine`) stay put; arms, head and weapon follow.
  Rendered check: `gunz-anim man idle --type 5 --time 0.2 --pitch 45 / -45`
  (`.local/shots/AnimCore/pitch_up.png`, `pitch_down.png`).

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

**Observed** (`system/gametypecfg.xml`): `GAMETYPE id` 0 deathmatch solo, 1 team deathmatch, 2 / 3
gladiator solo / team, 4 assassinate, 5 training, 9 `DEATHMATCH_TEAM2`, 10 duel (also 6 survival, 7 / 12
quest, 17 random weapon, 22 clan scrim: not playable offline here). Each lists `ROUNDS`, `LIMITTIME`
(attribute `sec`, but the labels read "10분": **minutes**; `-1` = unlimited) and `MAXPLAYERS` with one
`default="true"` each; the menu steppers use exactly these lists and defaults per mode (`Mode::limits`;
solo deathmatch 50 kills / 30 min, team 30 / 10, assassinate 30 / 10, id 9 70 / 40, duel 20 / 3 min).
Only the training default is ours (unlimited). `strings.xml` names: "Death match solo/team",
"Duel match", "Gladiator solo/team", "Assassinate", "Training"; id 9 is not named there, the mission
strings call team rounds "Elimination" (`MISSION_DES_WEEKLY_00251`), which is what the menu calls it.
`maps/<map>/<map>.rs.xml` has `spawn_solo_NNN` (Mansion 32), `spawn_team1_NNN` and `spawn_team2_NNN`
(16 each, two clusters at opposite ends) and `spawn_item_{solo|team}_*`; `spawn.xml` lists items per
`GAMETYPE id="solo"|"team"`. `system/blitzkrieg.xml` has `RESPAWN baseTime="8" invincibleTime="5"`
(Blitzkrieg only).

**Modes** (all single-player, bots fill the seats): *Deathmatch* (free for all), *Team DM* (player Red,
bots fill the smaller side, team kills count), *Gladiator* / *Team Gladiator* (the same with
melee-only loadouts: `Loadout.slots` is cut to the melee slot), *Elimination* (team rounds),
*Assassinate* (team rounds, one random VIP per team per round, tagged "[VIP]" in its name; a team is
out when its VIP dies), *Duel* (one-on-one rounds) and *Training* (no bots; four inert dummies 4-12 m
in front of the player, they respawn where they stood).

**Inferred** (not in the data): respawn delay 5 s (`--respawn`), spawn protection 3 s (`--protect`,
blinks, all damage ignored), round limit 180 s (`--round-time`, the duel's default `LIMITTIME`), ready
countdown 3 s (`--ready`), round-win screen 4 s, which side is `team1`/`team2`, no pickups in duel and
training. Which team's `spawn_team*` list a side uses is ours; a map without them falls back to all
`spawn*` dummies.

**Rounds** (Elimination, Assassinate, Duel): everybody respawns at round start at their side's spawns,
protected through the countdown; `Dead.respawn` of a dead actor is pinned at `session::HOLD`
(1e6 s) until the next round; a round ends when a side has nobody left (Assassinate: its VIP is
dead; duel: a fighter is dead), or at the round limit when the side with more actors alive (duel:
more health + armour) wins, a tie is a draw. The match ends when a side has `--kill-limit` round wins
(duel: a fighter has that many wins). While the player is dead the camera follows a living teammate
(`game::Spectate`; Space or click cycles) and the HUD says so. Duel: the queue starts with the player,
the first two fight (Red / Blue teams, so bots fight each other), the winner stays at the front, the
loser goes to the back, a draw sends both back; waiting actors are dead and hidden. The first round
of a headless run with `--at` or `--bots-ahead` leaves living actors where they are.

Spectating, spawn points and protection are carried by the shared components `game::Spectate`,
`game::SpawnAt`, `game::Protected`, `game::Vip` and the `game::NewRound` message (item spawners
refill). `--die-at S` kills the player at match time S (headless checks).

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
| zitem `snd_fire` | `blade_swing`, `we_{pistol,revolver,smg,shotgunpa,rifle,machinegun,rocket,grenade}_fire`, `swing` | all resolve except `swing` (one legacy NPC dagger) |
| zitem `snd_reload` | `we_{pistol,revolver,smg,shotgunpa,rifle,machinegun,rocket}_reload` | 7/7 |
| zitem `snd_dryfire` | `357magrevolver_dryfire`, `762arifle_dryfire` | 2/2 |

Distinct zitem sound names resolving: 18 of 19. 154 of the 155 weapon items resolve a fire sound
(114 carry `snd_fire`; the shop melee weapons carry none and use `blade_swing`, the sound of
the 12 legacy melee items with `snd_fire`; **inferred** default). `model/man/man01.xml` and
`woman01.xml` give 39 animations a `sound` attribute: `man_jump` (15, `jumpD`; files
`man_jump_mt_<material>`) and `fx_dash` (24, `tumble*`, file `fx_dash`). Footsteps are not tagged
in the animation XML: files `man_fs_{l,r}_mt_<material>` with materials `con drt met pnt snd snw
wat wod` (**inferred** naming: left/right foot, surface); the game plays `_mt_<material>` by looking up the polygon under the feet (see below). `system/animationevent.xml` only has `<NPC id>` entries (36 NPCs,
233 sound events, 97 distinct files, all resolve under `sound/quest/<monster>/` or
`sound/effect/`) with `AddAnimEvent eventtype="sound" filename beginframe`; it has nothing for
the player characters and is not used by `gunz-play`.

### Sound playback and feedback (`src/audio.rs`, `src/hud.rs`)

- **Surface** (**inferred** from names): a map material name's suffix `_mt_con/_drt/_met/_wod/_pnt/
  _snd/_snw/_wat/_gls/_fsh` selects the `_mt_<x>` file of footsteps, jump/land, bullet hit and shell
  drop; polygons without a suffix are concrete. Only the non-concrete polygons are indexed
  (Mansion 8 792, Dungeon 1 005).
- **Ambience**: the map's `AMBIENTSOUNDLIST` (`snd_amb_*` dummies; `effect.xml` type 6 = 3D loop,
  4/5 = 2D loop), at most 8 nearest loops play. Mansion has 3, Dungeon 74.
- **BGM**: `sound/bgm/` holds 16 files (15 ogg + `gunzmatching.mp3`), but no data file maps a
  map or mode to one (only `system/filelist.xml` lists them), so `gunz-play` plays none (**unknown**).
- **Visual**: bullet-hole / blood-mark decals from `sfx/*bulletmark*`, `sfx/blood-mark*` at the
  collision hit point along its normal; red damage-direction arcs; red edge vignette below low HP
  that pulses faster as health falls; hit marker, kill feed and centre kill notice.
- **Coverage** (headless run log): surface sets 35/35, voices 13/13, misc cues 16/16, zitem
  `snd_fire/reload/dryfire` 18/19 (`swing` has no file), weapon items with a fire sound 154/155,
  map ambiences 2/2 (Mansion) and 3/3 (Dungeon) resolve and decode.

Other names the HUD plays (the mapping is **inferred** from the names): `hitbody00` (bullet
hit), `blade_damage` (melee hit), `fx_myhit` (type 3, the player's own hit), `death01_a_male`
(death), `fx_respawn` (type 1, player respawn).

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
  `reloadtime` 3..10 is not seconds); a melee blow lands at 30 % of the slash clip
  and the next combo slash can start at 65 %.
- Layering (`Animator` from `src/anim.rs`): legs play `idle/run/runB/jump*`, the upper body plays
  `attackS`/`reload`/switch clips while moving, so shooting or reloading while running keeps the
  legs running; clips cross-fade (`blend_for`, 0.05-0.12 s, **inferred**: the data has no blend
  times); the spine takes the aim pitch. Tumbles and wall moves travel at the speed derived from
  the clip root motion (Fidelity), not a free constant.
- Action API (`game::ActionRequest`, `Acting`): other modules ask for a full-body clip with
  speed, root motion, movement lock and cancel window without touching `actor.rs`; `Acting.time`
  lets melee time its hit frame. A `Push` with vertical part >= `BLAST_PUSH` (5 m/s, uppercut,
  rocket, grenade) plays `blast` -> `blast_fall` -> `blast_drop`, lies `LIE` 0.35 s, then
  `blast_stand`; ordinary damage plays `damage`/`damage2`. Landing plays `jumpD`. Taunt key plays
  `taunt`. All clips of both sexes are parsed at startup (`ActorData`), never inside a match.
- Death camera: the corpse keeps its rotation (only living actors follow the camera yaw); the
  camera orbits it (`DEATH_ORBIT_PERIOD` 14 s, `DEATH_DIST` 3.8 m) plus the mouse.
- Frame times: `GUNZ_FRAMETIMES=1` (`src/perf.rs`) prints hitches and a p50/p99/max summary.
  Mansion, 3 bots, 60 s headless: play phase max 22.1 ms, 0 frames over 33 ms (the 50.7 ms
  frames are the first two, loading).

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
  actor controller. `Damage.item` names the weapon. HP/AP rule (**inferred**, no piercing ratio
  anywhere in `system/*.xml`; equipment `hp`/`ap` bonuses are the only AP/HP attributes):
  `absorb(v, amount, pierce)`: `pierce` of the hit goes to HP, the rest to AP, what AP cannot
  hold falls through to HP; `pierce` = melee 0.7, rifle/machine gun 0.6, shotgun 0.3, everything
  else 0.5 (`piercing`). `Vitals` of actors are 100 HP / 50 AP from `actor.rs`. Self damage is
  only taken from your own blasts; HP <= 0 inserts `Dead{respawn: 5 s}` (**inferred**), a
  suicide adds a death but no kill. `Protected` actors take nothing (no damage, blood, push).
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
  line-of-sight checks. Medikit/repair kit +50 HP/AP
  (**inferred**, no itempower); potions `itempower` per second for `damagetime` s (**observed**).
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
  walk 8 directions (steps up to `col::STEP` only), run off a ledge (`Drop`, <= 8 m fall, <= 6.5 m away;
  **inferred** acceptable because the port has no fall damage), jump (`Jump`, ledges and gaps where
  walking failed and nothing is at 1.4 m height) or **wall climb** (`Jump` whose takeoff is the point
  where the run-up first sees a wall within 2 m, on the 4 axis directions: jump so that the jump peaks
  at the wall, the controller's wall run (`runW`, 3.3 m in 0.6 s, `WALL_MIN_HEIGHT` of air under the
  feet) takes over and the wall-run's push into the wall carries the capsule over a ledge up to about
  4.5 m; this is how the 4.5 m balconies of Skirmish Hall and the 9 m levels above them link, 66 -> 100
  of 104 spawn routes). A link a bot is stuck on twice is marked broken and replanned. Jump steps
  carry the simulated takeoff point; the bot runs to it, jumps, and keeps walking at the landing node.
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
  within 0.7 m of the line to the target. Not done: butterfly, grenade throws.

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
- Mansion has no walkable way up to the y=13 floor and the y=6 wings from the hall: the stairs reach
  y=7 only; the y=13 floor is 6.0 m above y=7 along every rim (`GAP`), the y=6 wings end at a 17 m wide
  shaft at x -31..-14 (z -31..-20) whose floor is at y -2.8..-7 (8.8 m below the corridor, more than the
  8 m `DROP`; raising it to 14 m changed no route). A wall climb gains about 4.4 m (jump peak 1.1 m +
  wall run 3.3 m), a wall kick 1 m more; 6 m is out of reach, so those floors are drop-only. The old
  "invisible wall at x = +-16" is not the reason. 53/142 routes = every pair whose target is not above
  the start; the spawns on y=13 (6) and in the wings (36) are one-way places (bots there come down).
  Battle arena: 8 spawns sit in 6 m deep pits; Blitzkrieg: the team bases at y=9 are drop-only.
  Tried 0.25 m cells (door alignment): same Mansion result, 4x nodes, 1.7x slower routes; kept 0.5 m.

Routing table (throwaway probe: 2 spawn->spawn pairs per spawn point, a route counts when it ends within
2 m of the goal; before = links found lazily, no wall climb; after = this change):
```
map              before      after   | map             before      after
battle arena     105/142    111/142  | prison          142/142    142/142
blitzkrieg        64/80      64/80   | prison ii       140/140    140/140
castle            80/90      80/90   | ruin            146/146    146/146
catacomb           4/4        4/4    | shower room       4/4        4/4
citadel          102/102    102/102  | skirmishhall     66/104    100/104
classic town     126/140    132/140  | snow_town       136/152    140/152
dungeon          140/140    140/140  | stairway         58/88      62/88
factory           81/88      86/88   | station          92/104    100/104
garden            78/84      84/84   | test_a           96/96      96/96
hall               4/4        4/4    | test_b           96/96      96/96
halloween town   136/152    140/152  | town            126/140    132/140
high_haven        57/82      59/82   | weaponshop       64/64      64/64
island            78/92      82/92   | jail              4/4        4/4
lost shrine       69/88      76/88   | mansion          53/142     53/142
port              84/84      84/84   | TOTAL         2431/2794  2527/2794 (87.0 % -> 90.4 %)
```
Of the failed pairs (BAD=1 probe run before the last search change) 116 are "one way" (the reverse pair routes: a platform reachable only by dropping) and 143 "both
ways" (spawns in pits or on isolated islands, e.g. battle arena's 6 m pits); the 95 % target is not
reachable on Mansion, battle arena, blitzkrieg, high_haven, stairway, island, lost shrine without links
the controller cannot perform.

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
  the guard). `guard` held = `guard_start` -> `guard_idle`, blocks frontal (+-90 deg) melee with `guard_block1/2`,
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
