//! Quadro único BGRA (`FrameImage`) capturado de uma janela por ciclo.
//!
//! No legado (`ZenMonitor`), múltiplos detectores chamavam `PrintWindow` repetidamente
//! para o mesmo personagem no mesmo ciclo. No ZenGravity, o agente captura **um único**
//! `FrameImage` por ciclo para cada janela não minimizada e faz todos os recortes
//! (`crop`) em memória (canto do mapa/coordenadas, ícone do Helper, barra do Raven,
//! campo do Zen no inventário e número do HP).

/// Quadro BGRA (4 bytes por pixel, de cima para baixo) normalizado na origem do cliente.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrameImage {
    pub w: u32,
    pub h: u32,
    pub bgra: Vec<u8>,
}

impl FrameImage {
    /// Recorta a sub-região `[x, y, largura, altura]` em memória sem nova chamada ao SO.
    pub fn crop(&self, region: [i32; 4]) -> Option<FrameImage> {
        let [x, y, w, h] = region;
        if x < 0 || y < 0 || w <= 0 || h <= 0 || (x + w) as u32 > self.w || (y + h) as u32 > self.h
        {
            return None;
        }
        let mut out = Vec::with_capacity((w * h * 4) as usize);
        for row in y..y + h {
            let start = ((row as u32 * self.w + x as u32) * 4) as usize;
            out.extend_from_slice(&self.bgra[start..start + (w as usize) * 4]);
        }
        Some(FrameImage {
            w: w as u32,
            h: h as u32,
            bgra: out,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crops_in_memory_and_rejects_out_of_bounds() {
        let frame = FrameImage {
            w: 10,
            h: 10,
            bgra: vec![128; 10 * 10 * 4],
        };
        assert_eq!(frame.crop([2, 2, 4, 3]).map(|f| (f.w, f.h)), Some((4, 3)));
        assert!(frame.crop([-1, 0, 4, 3]).is_none());
        assert!(frame.crop([8, 8, 4, 3]).is_none());
    }
}
