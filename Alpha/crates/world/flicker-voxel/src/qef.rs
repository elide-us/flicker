//! Quadratic Error Function solver for dual-contouring vertex placement.
//!
//! Each surface crossing contributes a plane `n · (x − p) = 0` (point `p`,
//! unit normal `n`). The placed vertex minimizes the summed squared
//! distance to all such planes: `Σ (n_i · (x − p_i))²`. That is a 3×3
//! linear least-squares problem solved through the normal equations
//! `AᵀA x = Aᵀb`, where row `i` of `A` is `n_i` and `b_i = n_i · p_i`.
//!
//! `AᵀA` is rank-deficient when the crossing planes do not constrain all
//! three axes (a flat region constrains only one; an edge two). To stay
//! well-posed we add a small Tikhonov term biased toward the mass point
//! `m` (the average crossing position): minimize
//! `Σ (n_i · (x − p_i))² + λ‖x − m‖²`, giving `(AᵀA + λI) x = Aᵀb + λm`.
//! Constrained axes are essentially unchanged for small `λ`; unconstrained
//! axes fall back to the mass point — exactly the right behavior (a flat
//! patch's vertex sits at the crossing centroid, a sharp edge's vertex
//! slides to the crease).

/// Accumulates crossing planes and solves for the minimizing vertex.
#[derive(Clone, Debug, Default)]
pub struct Qef {
    /// Upper triangle of the symmetric `AᵀA`: `[a00, a01, a02, a11, a12, a22]`.
    ata: [f32; 6],
    /// `Aᵀb`.
    atb: [f32; 3],
    /// Sum of crossing positions, for the mass point.
    psum: [f32; 3],
    count: u32,
}

impl Qef {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of planes accumulated.
    #[must_use]
    pub fn count(&self) -> u32 {
        self.count
    }

    /// Add a crossing plane through `p` with normal `n`.
    pub fn add(&mut self, p: [f32; 3], n: [f32; 3]) {
        let d = n[0] * p[0] + n[1] * p[1] + n[2] * p[2];
        self.ata[0] += n[0] * n[0];
        self.ata[1] += n[0] * n[1];
        self.ata[2] += n[0] * n[2];
        self.ata[3] += n[1] * n[1];
        self.ata[4] += n[1] * n[2];
        self.ata[5] += n[2] * n[2];
        self.atb[0] += n[0] * d;
        self.atb[1] += n[1] * d;
        self.atb[2] += n[2] * d;
        self.psum[0] += p[0];
        self.psum[1] += p[1];
        self.psum[2] += p[2];
        self.count += 1;
    }

    /// The mass point — the average of accumulated crossing positions.
    /// Returns `[0, 0, 0]` if no planes were added.
    #[must_use]
    pub fn mass_point(&self) -> [f32; 3] {
        if self.count == 0 {
            return [0.0; 3];
        }
        let n = self.count as f32;
        [self.psum[0] / n, self.psum[1] / n, self.psum[2] / n]
    }

    /// Solve for the minimizing vertex, Tikhonov-regularized toward the
    /// mass point with strength `lambda`. Falls back to the mass point if
    /// the regularized system is still degenerate.
    #[must_use]
    pub fn solve(&self, lambda: f32) -> [f32; 3] {
        let m = self.mass_point();
        if self.count == 0 {
            return m;
        }
        // (AᵀA + λI) x = Aᵀb + λm
        let a00 = self.ata[0] + lambda;
        let a01 = self.ata[1];
        let a02 = self.ata[2];
        let a11 = self.ata[3] + lambda;
        let a12 = self.ata[4];
        let a22 = self.ata[5] + lambda;
        let b = [
            self.atb[0] + lambda * m[0],
            self.atb[1] + lambda * m[1],
            self.atb[2] + lambda * m[2],
        ];
        solve_sym3(a00, a01, a02, a11, a12, a22, b).unwrap_or(m)
    }
}

/// Solve a symmetric 3×3 system `A x = b` given the upper triangle of `A`.
/// Returns `None` if `A` is singular (determinant ≈ 0).
#[allow(clippy::too_many_arguments)]
fn solve_sym3(
    a00: f32,
    a01: f32,
    a02: f32,
    a11: f32,
    a12: f32,
    a22: f32,
    b: [f32; 3],
) -> Option<[f32; 3]> {
    // Cofactors of the symmetric matrix.
    let c00 = a11 * a22 - a12 * a12;
    let c01 = a02 * a12 - a01 * a22;
    let c02 = a01 * a12 - a02 * a11;
    let det = a00 * c00 + a01 * c01 + a02 * c02;
    if det.abs() < 1e-12 {
        return None;
    }
    let inv_det = 1.0 / det;
    let c11 = a00 * a22 - a02 * a02;
    let c12 = a02 * a01 - a00 * a12;
    let c22 = a00 * a11 - a01 * a01;
    // Inverse (symmetric) times b.
    Some([
        (c00 * b[0] + c01 * b[1] + c02 * b[2]) * inv_det,
        (c01 * b[0] + c11 * b[1] + c12 * b[2]) * inv_det,
        (c02 * b[0] + c12 * b[1] + c22 * b[2]) * inv_det,
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_plane_constrains_one_axis_rest_to_mass_point() {
        // One horizontal plane at y=128, normal up. Vertex Y → 128; X,Z →
        // the crossing's X,Z (mass point), since they are unconstrained.
        let mut q = Qef::new();
        q.add([10.5, 128.0, 20.5], [0.0, 1.0, 0.0]);
        let v = q.solve(0.01);
        assert!((v[0] - 10.5).abs() < 1e-3, "x={}", v[0]);
        assert!((v[1] - 128.0).abs() < 1e-3, "y={}", v[1]);
        assert!((v[2] - 20.5).abs() < 1e-3, "z={}", v[2]);
    }

    #[test]
    fn three_orthogonal_planes_meet_at_their_corner() {
        // Planes x=3, y=5, z=7 (axis normals). Vertex → (3, 5, 7).
        let mut q = Qef::new();
        q.add([3.0, 0.0, 0.0], [1.0, 0.0, 0.0]);
        q.add([0.0, 5.0, 0.0], [0.0, 1.0, 0.0]);
        q.add([0.0, 0.0, 7.0], [0.0, 0.0, 1.0]);
        let v = q.solve(0.0001);
        assert!((v[0] - 3.0).abs() < 1e-2, "x={}", v[0]);
        assert!((v[1] - 5.0).abs() < 1e-2, "y={}", v[1]);
        assert!((v[2] - 7.0).abs() < 1e-2, "z={}", v[2]);
    }

    #[test]
    fn empty_qef_returns_origin() {
        let q = Qef::new();
        assert_eq!(q.solve(0.01), [0.0, 0.0, 0.0]);
    }
}
