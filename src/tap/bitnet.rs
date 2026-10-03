//! # BitNet b1.58: Native 1.58-Bit Ternary Neural Engine (100% Safe Rust)
//!
//! Implements Microsoft's BitNet b1.58 architecture where all linear layer weights
//! are constrained to ternary values: `{-1, 0, +1}`.
//!
//! ## Mathematical Foundation
//! In BitNet b1.58, weight-activation multiplication is eliminated:
//! ```text
//! y_i = \gamma \times ( \sum_{w_{ij} = +1} x_j - \sum_{w_{ij} = -1} x_j )
//! ```
//! The matrix multiplication consists purely of **addition and subtraction**,
//! achieving extreme CPU/Edge efficiency and zero floating-point weight multiply latency.
//!
//! ## Safety & Purity
//! - **100% Safe Rust**: `#![forbid(unsafe_code)]` compliant.
//! - **Zero Placebo**: Computes mathematically verified ternary contractions.

use crate::error::{Error, Result};

/// Ternary weight value: {-1, 0, +1}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i8)]
pub enum TernaryVal {
    NegOne = -1,
    Zero = 0,
    PosOne = 1,
}

impl TernaryVal {
    /// Quantize a floating-point weight into {-1, 0, +1} given threshold
    #[inline]
    pub fn from_f32(val: f32, threshold: f32) -> Self {
        if val > threshold {
            TernaryVal::PosOne
        } else if val < -threshold {
            TernaryVal::NegOne
        } else {
            TernaryVal::Zero
        }
    }

    #[inline]
    pub fn as_i8(self) -> i8 {
        self as i8
    }

    #[inline]
    pub fn as_f32(self) -> f32 {
        match self {
            TernaryVal::NegOne => -1.0,
            TernaryVal::Zero => 0.0,
            TernaryVal::PosOne => 1.0,
        }
    }
}

/// A 1.58-bit Ternary Linear Projection Layer
#[derive(Debug, Clone)]
pub struct BitNetLinear {
    pub in_features: usize,
    pub out_features: usize,
    /// Ternary weights stored as flat row-major array of size `out_features * in_features`
    pub weights: Vec<TernaryVal>,
    /// Per-layer or per-channel floating point scale factor \gamma
    pub scale: f32,
    /// Optional bias vector of size `out_features`
    pub bias: Option<Vec<f32>>,
}

impl BitNetLinear {
    /// Create a new BitNetLinear layer from raw ternary weights
    pub fn new(
        in_features: usize,
        out_features: usize,
        weights: Vec<TernaryVal>,
        scale: f32,
        bias: Option<Vec<f32>>,
    ) -> Result<Self> {
        if weights.len() != in_features * out_features {
            return Err(Error::Internal(format!(
                "BitNetLinear weight size mismatch: expected {} * {} = {}, got {}",
                out_features,
                in_features,
                out_features * in_features,
                weights.len()
            )));
        }
        if let Some(ref b) = bias {
            if b.len() != out_features {
                return Err(Error::Internal(format!(
                    "BitNetLinear bias size mismatch: expected {}, got {}",
                    out_features,
                    b.len()
                )));
            }
        }
        Ok(Self {
            in_features,
            out_features,
            weights,
            scale,
            bias,
        })
    }

    /// Quantize a dense float matrix into a BitNet b1.58 linear layer
    pub fn from_float_weights(
        in_features: usize,
        out_features: usize,
        float_weights: &[f32],
        bias: Option<Vec<f32>>,
    ) -> Result<Self> {
        if float_weights.len() != in_features * out_features {
            return Err(Error::Internal(format!(
                "Float weights size mismatch: expected {}, got {}",
                in_features * out_features,
                float_weights.len()
            )));
        }

        // BitNet scaling: gamma = mean(abs(W))
        let abs_sum: f32 = float_weights.iter().map(|w| w.abs()).sum();
        let scale = if float_weights.is_empty() {
            1.0
        } else {
            (abs_sum / float_weights.len() as f32).max(1e-8)
        };

        // Quantize: W_ternary = RoundClip(W / scale, -1, 1)
        // With threshold around 0.5 * scale
        let threshold = 0.5 * scale;
        let ternary_weights: Vec<TernaryVal> = float_weights
            .iter()
            .map(|&w| TernaryVal::from_f32(w, threshold))
            .collect();

        Self::new(in_features, out_features, ternary_weights, scale, bias)
    }

    /// Forward pass: Multiplies input vector by ternary weights using purely additions and subtractions!
    ///
    /// Formula:
    /// `y_i = \gamma \times ( \sum_{w_{ij}=+1} x_j - \sum_{w_{ij}=-1} x_j ) + bias_i`
    pub fn forward(&self, input: &[f32]) -> Result<Vec<f32>> {
        if input.len() != self.in_features {
            return Err(Error::Internal(format!(
                "BitNetLinear input dim mismatch: expected {}, got {}",
                self.in_features,
                input.len()
            )));
        }

        let mut output = Vec::with_capacity(self.out_features);

        for row in 0..self.out_features {
            let row_offset = row * self.in_features;
            let mut acc: f32 = 0.0;

            // Pure addition & subtraction: ZERO float-multiplications with weights
            for col in 0..self.in_features {
                match self.weights[row_offset + col] {
                    TernaryVal::PosOne => acc += input[col],
                    TernaryVal::NegOne => acc -= input[col],
                    TernaryVal::Zero => {}
                }
            }

            let mut val = acc * self.scale;
            if let Some(ref bias) = self.bias {
                val += bias[row];
            }
            output.push(val);
        }

        Ok(output)
    }
}

/// RMSNorm normalization for BitNet layers
#[derive(Debug, Clone)]
pub struct BitNetRmsNorm {
    pub dim: usize,
    pub eps: f32,
    pub weight: Vec<f32>,
}

impl BitNetRmsNorm {
    pub fn new(dim: usize, eps: f32) -> Self {
        Self {
            dim,
            eps,
            weight: vec![1.0; dim],
        }
    }

    pub fn forward(&self, x: &[f32]) -> Result<Vec<f32>> {
        if x.len() != self.dim {
            return Err(Error::Internal(format!(
                "RMSNorm dimension mismatch: expected {}, got {}",
                self.dim,
                x.len()
            )));
        }
        let sum_sq: f32 = x.iter().map(|v| v * v).sum();
        let rms = (sum_sq / self.dim as f32 + self.eps).sqrt();
        let inv_rms = 1.0 / rms;

        let out = x
            .iter()
            .zip(self.weight.iter())
            .map(|(&xi, &wi)| xi * inv_rms * wi)
            .collect();
        Ok(out)
    }
}

/// BitNet b1.58 Multi-Layer Decision Block (RMSNorm -> BitNet Gate/Up -> SiLU -> BitNet Down)
#[derive(Debug, Clone)]
pub struct BitNetBlock {
    pub norm: BitNetRmsNorm,
    pub gate_proj: BitNetLinear,
    pub up_proj: BitNetLinear,
    pub down_proj: BitNetLinear,
}

impl BitNetBlock {
    pub fn new(hidden_dim: usize, intermediate_dim: usize) -> Result<Self> {
        let norm = BitNetRmsNorm::new(hidden_dim, 1e-6);

        // Deterministic ternary weight pattern for baseline initialization
        let make_weights = |rows, cols| -> Vec<TernaryVal> {
            (0..rows * cols)
                .map(|idx| match (idx * 31 + 7) % 3 {
                    0 => TernaryVal::Zero,
                    1 => TernaryVal::PosOne,
                    _ => TernaryVal::NegOne,
                })
                .collect()
        };

        let gate_proj = BitNetLinear::new(
            hidden_dim,
            intermediate_dim,
            make_weights(intermediate_dim, hidden_dim),
            0.15,
            None,
        )?;
        let up_proj = BitNetLinear::new(
            hidden_dim,
            intermediate_dim,
            make_weights(intermediate_dim, hidden_dim),
            0.15,
            None,
        )?;
        let down_proj = BitNetLinear::new(
            intermediate_dim,
            hidden_dim,
            make_weights(hidden_dim, intermediate_dim),
            0.15,
            None,
        )?;

        Ok(Self {
            norm,
            gate_proj,
            up_proj,
            down_proj,
        })
    }

    /// Forward pass through the BitNet SwiGLU block
    pub fn forward(&self, x: &[f32]) -> Result<Vec<f32>> {
        let normed = self.norm.forward(x)?;
        let gate = self.gate_proj.forward(&normed)?;
        let up = self.up_proj.forward(&normed)?;

        // SwiGLU: SiLU(gate) * up
        let mut intermediate = Vec::with_capacity(gate.len());
        for i in 0..gate.len() {
            let g = gate[i];
            let silu = g / (1.0 + (-g).exp());
            intermediate.push(silu * up[i]);
        }

        let down = self.down_proj.forward(&intermediate)?;

        // Residual connection
        let mut out = Vec::with_capacity(x.len());
        for i in 0..x.len() {
            out.push(x[i] + down[i]);
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ternary_val_quantization() {
        assert_eq!(TernaryVal::from_f32(1.2, 0.5), TernaryVal::PosOne);
        assert_eq!(TernaryVal::from_f32(-0.8, 0.5), TernaryVal::NegOne);
        assert_eq!(TernaryVal::from_f32(0.2, 0.5), TernaryVal::Zero);
    }

    #[test]
    fn test_bitnet_linear_pure_addition() {
        // 2 inputs, 2 outputs
        // row 0: [+1, -1] -> x0 - x1
        // row 1: [ 0, +1] -> x1
        let weights = vec![
            TernaryVal::PosOne,
            TernaryVal::NegOne,
            TernaryVal::Zero,
            TernaryVal::PosOne,
        ];
        let layer = BitNetLinear::new(2, 2, weights, 1.0, None).unwrap();

        let input = vec![5.0, 3.0];
        let output = layer.forward(&input).unwrap();

        // row 0: 5.0 - 3.0 = 2.0
        // row 1: 3.0 = 3.0
        assert_eq!(output, vec![2.0, 3.0]);
    }

    #[test]
    fn test_bitnet_rms_norm() {
        let norm = BitNetRmsNorm::new(4, 1e-6);
        let x = vec![1.0, 1.0, 1.0, 1.0];
        let out = norm.forward(&x).unwrap();
        // RMS of [1,1,1,1] is ~1.0, so out ~ [1,1,1,1]
        for v in out {
            assert!((v - 1.0).abs() < 1e-3);
        }
    }

    #[test]
    fn test_bitnet_block_forward() {
        let block = BitNetBlock::new(8, 16).unwrap();
        let x = vec![0.5; 8];
        let out = block.forward(&x).unwrap();
        assert_eq!(out.len(), 8);
        for v in out {
            assert!(!v.is_nan());
        }
    }
}
