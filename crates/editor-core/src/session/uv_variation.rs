use super::*;
use sha2::{Digest, Sha256};

impl Session {
    pub fn vary_face_uv(
        &mut self,
        actor: Actor,
        faces: &[FaceId],
        seed: u32,
        max_shift_u: f64,
        max_shift_v: f64,
        preview: bool,
    ) -> Result<usize, String> {
        self.ensure_not_closing()?;
        if [max_shift_u, max_shift_v]
            .into_iter()
            .any(|value| !value.is_finite() || value < 0.0)
        {
            return Err("Maximum U/V shifts must be finite and nonnegative texel values".into());
        }
        if faces.is_empty() {
            return Err("Select at least one face".into());
        }
        let mut edits = Vec::new();
        for face in faces.iter().copied().collect::<BTreeSet<_>>() {
            let side = surface::side(&self.document, face)
                .ok_or_else(|| format!("solid {} has no side {}", face.solid, face.side))?;
            let mut hash = Sha256::new();
            hash.update(b"HammeRR face UV offsets v1\0");
            hash.update(seed.to_le_bytes());
            hash.update(face.solid.to_le_bytes());
            hash.update(face.side.to_le_bytes());
            let digest = hash.finalize();
            let u = u32::from_le_bytes(digest[..4].try_into().unwrap());
            let v = u32::from_le_bytes(digest[4..8].try_into().unwrap());
            edits.push((
                face,
                SurfaceEdit {
                    shift_u: (max_shift_u != 0.0)
                        .then(|| side.u_axis.offset + signed_fraction(u) * max_shift_u),
                    shift_v: (max_shift_v != 0.0)
                        .then(|| side.v_axis.offset + signed_fraction(v) * max_shift_v),
                    ..Default::default()
                },
            ));
        }
        if max_shift_u == 0.0 && max_shift_v == 0.0 {
            return Ok(0);
        }
        self.preview_or_apply_faces(actor, edits, preview)
    }
}

fn signed_fraction(value: u32) -> f64 {
    f64::from(value) / f64::from(u32::MAX) * 2.0 - 1.0
}
