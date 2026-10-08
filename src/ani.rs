//! `.elu.ani` animation files (RealSpace "ELU animation"). Layouts: `docs/formats.md`.
//! Values are returned as stored (left-handed, Z up, centimetres, D3D row-major matrices);
//! rotation keys of every version are normalised to quaternions `[x, y, z, w]`.

use std::io::{self, ErrorKind};

const MAGIC: u32 = 0x0107_F060;
/// Pre-`0x1001` files (21 in model/lo): no visibility track, axis-angle rotations.
const VER_OLD: u32 = 0x12;
/// Has visibility tracks, axis-angle rotations.
const VER_VIS: u32 = 0x1001;
/// Same layout as `0x1001`, quaternion rotations.
const VER_QUAT: u32 = 0x1003;

/// 3ds Max ticks per frame; animations run at [`FPS`].
pub const TICKS_PER_FRAME: u32 = 160;
pub const FPS: f32 = 30.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Per-vertex positions per frame (`Node::vertex`).
    Vertex,
    /// Skeleton animation: `base` world matrix plus `pos`/`rot` keys relative to the parent.
    Bone,
    /// Whole-node transform keys (`Node::tm`).
    Transform,
}

#[derive(Clone, Copy, Debug)]
pub struct Key<T> {
    pub tick: u32,
    pub value: T,
}

#[derive(Clone, Debug)]
pub struct VertexTrack {
    pub vertex_count: usize,
    pub ticks: Vec<u32>,
    /// `ticks.len() * vertex_count` positions, frame-major, in the node's local space.
    pub positions: Vec<[f32; 3]>,
}

#[derive(Clone, Debug, Default)]
pub struct Node {
    pub name: String,
    /// Bone files only: the node's world (bind-time) matrix, row-major, translation in row 3.
    pub base: Option<[f32; 16]>,
    /// Bone files: translation relative to the parent node.
    pub pos: Vec<Key<[f32; 3]>>,
    /// Bone files: rotation relative to the parent node, quaternion `[x, y, z, w]`.
    pub rot: Vec<Key<[f32; 4]>>,
    /// Transform files: node matrix, row-major.
    pub tm: Vec<Key<[f32; 16]>>,
    /// Visibility / alpha multiplier in `0..=1`.
    pub vis: Vec<Key<f32>>,
    pub vertex: Option<VertexTrack>,
}

#[derive(Clone, Debug)]
pub struct Ani {
    pub version: u32,
    /// Index of the last frame (`max_frame * TICKS_PER_FRAME` ticks).
    pub max_frame: u32,
    pub kind: Kind,
    pub nodes: Vec<Node>,
}

fn bad(msg: impl Into<String>) -> io::Error {
    io::Error::new(ErrorKind::InvalidData, msg.into())
}

struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl Reader<'_> {
    fn bytes(&mut self, n: usize) -> io::Result<&[u8]> {
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
    fn u32(&mut self) -> io::Result<u32> {
        Ok(u32::from_le_bytes(self.bytes(4)?.try_into().unwrap()))
    }
    fn f32(&mut self) -> io::Result<f32> {
        Ok(f32::from_le_bytes(self.bytes(4)?.try_into().unwrap()))
    }
    fn vec<const N: usize>(&mut self) -> io::Result<[f32; N]> {
        let mut v = [0.0; N];
        for x in &mut v {
            *x = self.f32()?;
        }
        Ok(v)
    }
    /// Element count, rejected when `min_size` bytes per element cannot fit in the rest.
    fn count(&mut self, min_size: usize) -> io::Result<usize> {
        let n = self.u32()? as usize;
        if n.saturating_mul(min_size) > self.buf.len() - self.pos {
            return Err(bad(format!("count {n} at {} exceeds data", self.pos - 4)));
        }
        Ok(n)
    }
    /// Fixed 40-byte NUL-terminated name.
    fn name(&mut self) -> io::Result<String> {
        let b = self.bytes(40)?;
        let len = b
            .iter()
            .position(|&c| c == 0)
            .ok_or_else(|| bad("unterminated node name"))?;
        Ok(String::from_utf8_lossy(&b[..len]).into_owned())
    }
    /// Count, then `count` keys of (value, tick); `size` is the byte size of one key.
    fn keys<T>(
        &mut self,
        size: usize,
        mut value: impl FnMut(&mut Self) -> io::Result<T>,
    ) -> io::Result<Vec<Key<T>>> {
        let n = self.count(size)?;
        let mut v = Vec::with_capacity(n);
        for _ in 0..n {
            let value = value(self)?;
            v.push(Key {
                tick: self.u32()?,
                value,
            });
        }
        Ok(v)
    }
}

/// Axis-angle (radians) to quaternion `[x, y, z, w]`.
fn axis_angle([x, y, z, a]: [f32; 4]) -> [f32; 4] {
    let (s, c) = (a * 0.5).sin_cos();
    [x * s, y * s, z * s, c]
}

fn node(r: &mut Reader, kind: Kind, version: u32) -> io::Result<Node> {
    let mut n = Node { name: r.name()?, ..Node::default() };
    match kind {
        Kind::Bone => {
            n.base = Some(r.vec()?);
            n.pos = r.keys(16, |r| r.vec())?;
            let quat = version == VER_QUAT;
            n.rot = r.keys(20, |r| r.vec::<4>().map(|q| if quat { q } else { axis_angle(q) }))?;
        }
        Kind::Transform => n.tm = r.keys(68, |r| r.vec())?,
        Kind::Vertex => {
            let frames = r.count(4)?;
            let vertex_count = r.u32()? as usize;
            let ticks = (0..frames).map(|_| r.u32()).collect::<io::Result<Vec<_>>>()?;
            let total = frames
                .checked_mul(vertex_count)
                .filter(|t| t.saturating_mul(12) <= r.buf.len() - r.pos)
                .ok_or_else(|| bad("vertex data exceeds file"))?;
            let positions = (0..total).map(|_| r.vec()).collect::<io::Result<Vec<_>>>()?;
            n.vertex = Some(VertexTrack { vertex_count, ticks, positions });
        }
    }
    if version != VER_OLD {
        n.vis = r.keys(8, |r| r.f32())?;
    }
    Ok(n)
}

pub fn load(bytes: &[u8]) -> io::Result<Ani> {
    let mut r = Reader { buf: bytes, pos: 0 };
    let magic = r.u32()?;
    let version = r.u32()?;
    if magic != MAGIC || !matches!(version, VER_OLD | VER_VIS | VER_QUAT) {
        return Err(bad(format!(
            "ani: magic {magic:#x} version {version:#x} unsupported"
        )));
    }
    let max_frame = r.u32()?;
    let node_count = r.count(40)?;
    let kind = match r.u32()? {
        1 => Kind::Vertex,
        2 => Kind::Bone,
        3 => Kind::Transform,
        k => return Err(bad(format!("ani: unknown kind {k}"))),
    };
    let mut nodes = Vec::with_capacity(node_count);
    for i in 0..node_count {
        let start = r.pos;
        nodes.push(node(&mut r, kind, version).map_err(|e| bad(format!("ani node {i} at {start}: {e}")))?);
    }
    if r.pos != bytes.len() {
        return Err(bad(format!("ani: {} trailing bytes", bytes.len() - r.pos)));
    }
    Ok(Ani {
        version,
        max_frame,
        kind,
        nodes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn put(b: &mut Vec<u8>, w: &[u32]) {
        for x in w {
            b.extend_from_slice(&x.to_le_bytes());
        }
    }

    /// One bone with a single axis-angle rotation key in a 0x1001 file: 90 degrees about +Z.
    #[test]
    fn axis_angle_bone() {
        let mut b = Vec::new();
        put(&mut b, &[MAGIC, VER_VIS, 0, 1, 2]);
        let mut name = b"Bip01".to_vec();
        name.resize(40, 0);
        b.extend(name);
        put(&mut b, &[0; 16]); // base matrix
        put(&mut b, &[0]); // pos keys
        put(
            &mut b,
            &[
                1,
                0,
                0,
                1.0f32.to_bits(),
                std::f32::consts::FRAC_PI_2.to_bits(),
                0,
            ],
        ); // rot key
        put(&mut b, &[0]); // vis keys
        let ani = load(&b).unwrap();
        let q = ani.nodes[0].rot[0].value;
        let h = std::f32::consts::FRAC_1_SQRT_2;
        assert!(
            q[0].abs() < 1e-6
                && q[1].abs() < 1e-6
                && (q[2] - h).abs() < 1e-6
                && (q[3] - h).abs() < 1e-6
        );
        assert!(load(&b[..b.len() - 1]).is_err());
    }
}
