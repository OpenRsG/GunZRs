//! RealSpace2 maps (`.RS` v7 + `.RS.xml` + `.RS.lm` v3). Layouts: `docs/formats.md`.
//! Coordinates are returned as stored: left-handed, Z up, centimetre-ish units.

use crate::mrs::Vfs;
use std::io::{self, ErrorKind};

const RS_ID: u32 = 0x1234_5678;
const RS_VERSION: u32 = 7;
const LM_ID: u32 = 0x3067_1804;
const LM_VERSION: u32 = 3;

#[derive(Clone, Copy, Debug)]
pub struct Vertex {
    pub pos: [f32; 3],
    pub normal: [f32; 3],
    pub uv: [f32; 2],
    pub lm_uv: [f32; 2],
}

/// Convex render polygon (a triangle fan) from the octree leaves, in file order.
#[derive(Clone, Copy, Debug)]
pub struct Polygon {
    pub material: u32,
    pub flags: u32,
    pub first: u32,
    pub count: u32,
    pub lightmap: u32,
}

#[derive(Clone, Debug, Default)]
pub struct Material {
    pub name: String,
    pub diffuse_map: Option<String>,
    pub opacity: bool,
    pub alpha_test: bool,
    pub additive: bool,
    pub two_sided: bool,
}

#[derive(Clone, Debug)]
pub struct Dummy {
    pub name: String,
    pub pos: [f32; 3],
    pub dir: [f32; 3],
}

pub struct Map {
    /// VFS directory holding the map, e.g. `maps/mansion/`.
    pub dir: String,
    pub materials: Vec<Material>,
    pub vertices: Vec<Vertex>,
    pub polygons: Vec<Polygon>,
    /// Raw BMP files; empty when the map has no `.RS.lm` (every polygon then has `lightmap` 0).
    pub lightmaps: Vec<Vec<u8>>,
    pub dummies: Vec<Dummy>,
    /// `OBJECTLIST/OBJECT/@name`: `.elu` props (VFS names relative to `dir`, case as written).
    pub objects: Vec<String>,
}

fn bad(msg: impl Into<String>) -> io::Error {
    io::Error::new(ErrorKind::InvalidData, msg.into())
}

/// Bounds-checked little-endian reader.
struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn bytes(&mut self, n: usize) -> io::Result<&'a [u8]> {
        let end = self
            .pos
            .checked_add(n)
            .filter(|&e| e <= self.buf.len())
            .ok_or_else(|| {
                bad(format!(
                    "read of {n} bytes at {} overruns {}",
                    self.pos,
                    self.buf.len()
                ))
            })?;
        let s = &self.buf[self.pos..end];
        self.pos = end;
        Ok(s)
    }
    fn u8(&mut self) -> io::Result<u8> {
        Ok(self.bytes(1)?[0])
    }
    fn u32(&mut self) -> io::Result<u32> {
        Ok(u32::from_le_bytes(self.bytes(4)?.try_into().unwrap()))
    }
    fn f32(&mut self) -> io::Result<f32> {
        Ok(f32::from_le_bytes(self.bytes(4)?.try_into().unwrap()))
    }
    /// Non-negative i32 count, capped against the remaining bytes (`min_size` per element).
    fn count(&mut self, min_size: usize) -> io::Result<usize> {
        let n = self.u32()? as usize;
        if n.saturating_mul(min_size) > self.buf.len() - self.pos {
            return Err(bad(format!("count {n} at {} exceeds data", self.pos - 4)));
        }
        Ok(n)
    }
    fn vec<const N: usize>(&mut self) -> io::Result<[f32; N]> {
        let mut v = [0.0; N];
        for x in &mut v {
            *x = self.f32()?;
        }
        Ok(v)
    }
    fn cstr(&mut self) -> io::Result<String> {
        let rest = &self.buf[self.pos..];
        let len = rest
            .iter()
            .position(|&b| b == 0)
            .ok_or_else(|| bad("unterminated string"))?;
        self.pos += len + 1;
        Ok(String::from_utf8_lossy(&rest[..len]).into_owned())
    }
    fn header(&mut self, id: u32, version: u32, what: &str) -> io::Result<()> {
        let (i, v) = (self.u32()?, self.u32()?);
        if i != id || v != version {
            return Err(bad(format!(
                "{what}: id {i:#x} version {v}, expected {id:#x} v{version}"
            )));
        }
        Ok(())
    }
}

/// Finds the `.rs` file for a map directory name such as `Mansion`, `Dungeon_new`, the quest
/// map `Mansion_Hall1` or the challenge-quest map `G_Easy_2` (`maps/`, `quest/maps/` and
/// `challengequest/maps/` hold distinct directory names).
pub fn find_rs(vfs: &Vfs, map: &str) -> Option<String> {
    let name = map.to_ascii_lowercase();
    ["maps", "quest/maps", "challengequest/maps"]
        .into_iter()
        .find_map(|root| {
            let dir = format!("{root}/{name}/");
            let exact = format!("{dir}{name}.rs");
            if vfs.exists(&exact) {
                return Some(exact);
            }
            vfs.paths()
                .filter(|p| p.starts_with(&dir) && p.ends_with(".rs"))
                .min()
                .map(str::to_string)
        })
}

pub fn load(vfs: &Vfs, rs_path: &str) -> io::Result<Map> {
    let dir = rs_path
        .rsplit_once('/')
        .map_or(String::new(), |(d, _)| format!("{d}/"));
    let rs = vfs.read(rs_path)?;
    let mut r = Reader { buf: &rs, pos: 0 };
    r.header(RS_ID, RS_VERSION, rs_path)?;

    let names = (0..r.count(1)?)
        .map(|_| r.cstr())
        .collect::<io::Result<Vec<_>>>()?;

    // Source (pre-split) convex polygons: used for collision/lightmap baking, not drawn.
    let (convex, _convex_vertices) = (r.count(36)?, r.u32()?);
    for _ in 0..convex {
        r.bytes(4 + 4 + 16 + 4)?; // material, flags, plane, area
        let n = r.count(24)?;
        r.bytes(n * 24)?; // positions + normals
    }
    // BSP tree counts (tree itself lives in `.RS.bsp`), then octree counts.
    r.bytes(16)?;
    let (nodes, polys, verts, _indices) =
        (r.u32()?, r.u32()? as usize, r.u32()? as usize, r.u32()?);

    let mut vertices = Vec::with_capacity(verts.min(1 << 24));
    let mut polygons = Vec::with_capacity(polys.min(1 << 24));
    let mut seen = 0u32;
    read_node(&mut r, &mut vertices, &mut polygons, &mut seen, names.len())?;
    if r.pos != rs.len() || seen != nodes || polygons.len() != polys || vertices.len() != verts {
        return Err(bad(format!(
            "{rs_path}: octree mismatch (nodes {seen}/{nodes}, polygons {}/{polys}, vertices {}/{verts}, {} trailing bytes)",
            polygons.len(),
            vertices.len(),
            rs.len() - r.pos
        )));
    }

    // Only the developer test plane `challengequest/maps/2` ships without `.RS.lm` (its export
    // log shows BSP export only, no lighting pass): it loads without lightmaps.
    let lm_path = format!("{rs_path}.lm");
    let lightmaps = if vfs.exists(&lm_path) {
        read_lightmaps(&vfs.read(&lm_path)?, &mut vertices, &mut polygons, convex)?
    } else {
        Vec::new()
    };
    let xml = vfs.read(&format!("{rs_path}.xml"))?;
    let (mut materials, dummies, objects) = read_xml(&String::from_utf8_lossy(&xml), &names)?;
    materials.push(Material::default());
    Ok(Map {
        dir,
        materials,
        vertices,
        polygons,
        lightmaps,
        dummies,
        objects,
    })
}

fn read_node(
    r: &mut Reader,
    verts: &mut Vec<Vertex>,
    polys: &mut Vec<Polygon>,
    seen: &mut u32,
    materials: usize,
) -> io::Result<()> {
    *seen += 1;
    r.bytes(24 + 16)?; // bounding box, split plane
    for _ in 0..2 {
        if r.u8()? != 0 {
            read_node(r, verts, polys, seen, materials)?;
        }
    }
    for _ in 0..r.count(28)? {
        let (material, _convex, flags, n) = (r.u32()?, r.u32()?, r.u32()?, r.count(40)?);
        // -1 means "no material"; it maps to the default material appended after the named ones.
        let material = if material == u32::MAX {
            materials as u32
        } else {
            material
        };
        if material as usize > materials || n < 3 {
            return Err(bad(format!(
                "polygon at {}: material {material}, {n} vertices",
                r.pos
            )));
        }
        polys.push(Polygon {
            material,
            flags,
            first: verts.len() as u32,
            count: n as u32,
            lightmap: 0,
        });
        for _ in 0..n {
            let (pos, normal, uv) = (r.vec::<3>()?, r.vec::<3>()?, r.vec::<2>()?);
            r.bytes(8)?; // second UV slot; lightmap UVs come from `.lm`
            verts.push(Vertex {
                pos,
                normal,
                uv,
                lm_uv: [0.0; 2],
            });
        }
        r.bytes(12)?; // polygon normal
    }
    Ok(())
}

fn read_lightmaps(
    lm: &[u8],
    verts: &mut [Vertex],
    polys: &mut [Polygon],
    convex: usize,
) -> io::Result<Vec<Vec<u8>>> {
    let mut r = Reader { buf: lm, pos: 0 };
    r.header(LM_ID, LM_VERSION, "lightmap")?;
    let (sources, _nodes) = (r.u32()? as usize, r.u32()?);
    if sources != convex {
        return Err(bad(format!(
            "lightmap built for {sources} polygons, map has {convex}"
        )));
    }
    let images = (0..r.count(4)?)
        .map(|_| {
            let n = r.count(1)?;
            Ok(r.bytes(n)?.to_vec())
        })
        .collect::<io::Result<Vec<_>>>()?;
    // `lightmap_index[k]` belongs to octree polygon `order[k]` (UVs stay in file order).
    let order = (0..polys.len())
        .map(|_| r.u32())
        .collect::<io::Result<Vec<_>>>()?;
    for &i in &order {
        let lightmap = r.u32()?;
        let p = polys
            .get_mut(i as usize)
            .ok_or_else(|| bad(format!("lightmap order entry {i} of {}", order.len())))?;
        if lightmap as usize >= images.len() {
            return Err(bad(format!(
                "polygon lightmap index {lightmap} of {}",
                images.len()
            )));
        }
        p.lightmap = lightmap;
    }
    for v in verts.iter_mut() {
        v.lm_uv = r.vec::<2>()?;
    }
    if r.pos != lm.len() {
        return Err(bad(format!(
            "lightmap: {} trailing bytes",
            lm.len() - r.pos
        )));
    }
    Ok(images)
}

fn floats<const N: usize>(text: Option<&str>) -> Option<[f32; N]> {
    let mut it = text?.split_whitespace().map(|s| s.parse::<f32>());
    let mut v = [0.0; N];
    for x in &mut v {
        *x = it.next()?.ok()?;
    }
    Some(v)
}

type MapXml = (Vec<Material>, Vec<Dummy>, Vec<String>);

fn read_xml(text: &str, names: &[String]) -> io::Result<MapXml> {
    let doc = roxmltree::Document::parse(text).map_err(|e| bad(format!("map xml: {e}")))?;
    fn child<'a, 'i>(n: roxmltree::Node<'a, 'i>, tag: &str) -> Option<roxmltree::Node<'a, 'i>> {
        n.children().find(|c| c.has_tag_name(tag))
    }
    let mut materials: Vec<Material> = names
        .iter()
        .map(|n| Material {
            name: n.clone(),
            ..Default::default()
        })
        .collect();
    let mut dummies = Vec::new();
    let mut objects = Vec::new();
    for node in doc.descendants() {
        if node.has_tag_name("MATERIAL") {
            let Some(m) = materials
                .iter_mut()
                .find(|m| Some(m.name.as_str()) == node.attribute("name"))
            else {
                continue;
            };
            m.diffuse_map = child(node, "DIFFUSEMAP")
                .and_then(|c| c.text())
                .map(str::to_string);
            m.opacity = child(node, "USEOPACITY").is_some();
            m.alpha_test = child(node, "USEALPHATEST").is_some();
            m.additive = child(node, "ADDITIVE").is_some();
            m.two_sided = child(node, "TWOSIDED").is_some();
        } else if node.has_tag_name("DUMMY") {
            let pos = floats(child(node, "POSITION").and_then(|c| c.text()));
            let dir = floats(child(node, "DIRECTION").and_then(|c| c.text()));
            if let (Some(name), Some(pos), Some(dir)) = (node.attribute("name"), pos, dir) {
                dummies.push(Dummy {
                    name: name.to_string(),
                    pos,
                    dir,
                });
            }
        } else if node.has_tag_name("OBJECT") {
            objects.extend(node.attribute("name").map(str::to_string));
        }
    }
    Ok((materials, dummies, objects))
}
