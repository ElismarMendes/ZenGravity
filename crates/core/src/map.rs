//!Mapas do MU Online (`WorldId`), descriptografia de `Data\WorldN\EncTerrainN.att`,
//! grid 256×256 (`TerrainGrid`), zonas seguras, cálculo de distância BFS e Pathfinding A*.

pub const MAP_SIZE: usize = 256;
pub const ATTR_SAFEZONE: u8 = 0x01;
pub const ATTR_NOMOVE: u8 = 0x04;
pub const ATTR_NOGROUND: u8 = 0x08;

/// Chave de 16 bytes do `MapFileDecrypt` do cliente MU Online (`ZzzLodTerrain.cpp` / `_crypt.h`).
const TERRAIN_KEY: [u8; 16] = [
    0xD1, 0x73, 0x52, 0xF6, 0xD2, 0x9A, 0xCB, 0x27, 0x3E, 0xAF, 0x59, 0x31, 0x37, 0xB3, 0xE7, 0xA2,
];
const BUX_KEY: [u8; 3] = [0xFC, 0xCF, 0xAB];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TerrainParseError {
    UnexpectedSize(usize),
    InvalidHeader([u8; 4]),
    UnknownWorld(u8),
}

impl std::fmt::Display for TerrainParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TerrainParseError::UnexpectedSize(n) => {
                write!(
                    f,
                    "Tamanho inesperado de EncTerrain: {n} bytes (esperado 65540 ou 131076)"
                )
            }
            TerrainParseError::InvalidHeader(h) => {
                write!(
                    f,
                    "Cabeçalho inválido de EncTerrain: {h:?} (esperado [0, world, 255, 255])"
                )
            }
            TerrainParseError::UnknownWorld(w) => {
                write!(f, "WorldId {w} não mapeado na tabela principal")
            }
        }
    }
}

fn decrypt_terrain_bytes(raw: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(raw.len());
    let mut sub: u8 = 0x5E;
    for (n, &c) in raw.iter().enumerate() {
        let v = (TERRAIN_KEY[n % 16] ^ c).wrapping_sub(sub);
        sub = c.wrapping_add(0x3D);
        out.push(v ^ BUX_KEY[n % 3]);
    }
    out
}

/// Posição do mundo em float (lida da lista de objetos da RAM) convertida para coordenada de tile (`0..=255`).
pub fn world_float_to_tile(v: f32) -> i32 {
    (v / 100.0).floor() as i32
}

/// Identificadores de mundos (`WorldN`) validados no cliente MU Online.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum WorldId {
    Lorencia = 1,
    Dungeon = 2,
    Devias = 3,
    Noria = 4,
    LostTower = 5,
    Arena = 7,
    Atlans = 8,
    Tarkan = 9,
    Icarus = 11,
    ValleyOfLoren = 31,
    Aida = 34,
    Crywolf = 35,
    Kanturu = 38,
}

impl WorldId {
    pub const ALL: &'static [WorldId] = &[
        WorldId::Lorencia,
        WorldId::Dungeon,
        WorldId::Devias,
        WorldId::Noria,
        WorldId::LostTower,
        WorldId::Arena,
        WorldId::Atlans,
        WorldId::Tarkan,
        WorldId::Icarus,
        WorldId::ValleyOfLoren,
        WorldId::Aida,
        WorldId::Crywolf,
        WorldId::Kanturu,
    ];

    pub fn code(self) -> u8 {
        self as u8
    }

    pub fn from_code(code: u8) -> Option<Self> {
        Some(match code {
            1 => WorldId::Lorencia,
            2 => WorldId::Dungeon,
            3 => WorldId::Devias,
            4 => WorldId::Noria,
            5 => WorldId::LostTower,
            7 => WorldId::Arena,
            8 => WorldId::Atlans,
            9 => WorldId::Tarkan,
            11 => WorldId::Icarus,
            31 => WorldId::ValleyOfLoren,
            34 => WorldId::Aida,
            35 => WorldId::Crywolf,
            38 => WorldId::Kanturu,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            WorldId::Lorencia => "Lorencia",
            WorldId::Dungeon => "Dungeon",
            WorldId::Devias => "Devias",
            WorldId::Noria => "Noria",
            WorldId::LostTower => "Lost Tower",
            WorldId::Arena => "Arena",
            WorldId::Atlans => "Atlans",
            WorldId::Tarkan => "Tarkan",
            WorldId::Icarus => "Icarus",
            WorldId::ValleyOfLoren => "Valley of Loren",
            WorldId::Aida => "Aida",
            WorldId::Crywolf => "Crywolf Fortress",
            WorldId::Kanturu => "Kanturu",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        let lower = name.trim().to_ascii_lowercase();
        match lower.as_str() {
            "lorencia" => Some(WorldId::Lorencia),
            "dungeon" => Some(WorldId::Dungeon),
            "devias" => Some(WorldId::Devias),
            "noria" => Some(WorldId::Noria),
            "lost tower" | "losttower" => Some(WorldId::LostTower),
            "arena" | "stadium" => Some(WorldId::Arena),
            "atlans" => Some(WorldId::Atlans),
            "tarkan" => Some(WorldId::Tarkan),
            "icarus" => Some(WorldId::Icarus),
            "valley of loren" => Some(WorldId::ValleyOfLoren),
            "aida" => Some(WorldId::Aida),
            "crywolf" | "crywolf fortress" => Some(WorldId::Crywolf),
            "kanturu" | "kanturu 1" | "kanturu ruins" => Some(WorldId::Kanturu),
            _ => None,
        }
    }

    /// Caminho relativo dentro da pasta `Data` do cliente (ex.: `World38\EncTerrain38.att`).
    pub fn client_terrain_relative_path(self) -> String {
        let n = self.code();
        format!("World{n}\\EncTerrain{n}.att")
    }

    /// Pontos de chegada conhecidos do teleporte pelo menu M quando o mapa não possui Safe Zone tradicional.
    pub fn arrival_entry_tiles(self) -> &'static [(i32, i32)] {
        match self {
            WorldId::Dungeon => &[(108, 247), (233, 126), (3, 84)],
            WorldId::LostTower => &[(208, 78), (86, 166), (42, 245)],
            WorldId::Atlans => &[(22, 22)],
            WorldId::Tarkan => &[(195, 65)],
            WorldId::Icarus => &[(15, 13)],
            WorldId::Arena => &[(62, 115)],
            WorldId::Kanturu => &[(34, 217)],
            WorldId::Lorencia => &[(138, 128)],
            WorldId::Devias => &[(218, 63)],
            WorldId::Noria => &[(175, 108)],
            WorldId::Aida => &[(85, 14)],
            WorldId::Crywolf => &[(120, 25)],
            WorldId::ValleyOfLoren => &[(90, 40)],
        }
    }

    /// Retângulo principal aproximado da Safe Zone quando o `.att` não estiver carregado.
    pub fn safezone_bounds(self) -> Option<(i32, i32, i32, i32)> {
        match self {
            WorldId::Lorencia => Some((130, 118, 152, 138)),
            WorldId::Devias => Some((195, 32, 228, 68)),
            WorldId::Noria => Some((165, 90, 195, 122)),
            WorldId::Kanturu => Some((16, 202, 47, 213)),
            WorldId::Aida => Some((76, 8, 96, 24)),
            WorldId::Arena => Some((54, 110, 72, 128)),
            _ => None,
        }
    }
}

/// Grid 256×256 de um mapa com atributos de terreno e cálculo de rota A*.
#[derive(Debug, Clone, PartialEq)]
pub struct TerrainGrid {
    pub world: WorldId,
    pub raw_world_number: u8,
    pub from_client_att: bool,
    tiles: Vec<u8>,
}

impl TerrainGrid {
    /// Descriptografa e carrega um arquivo `Data\WorldN\EncTerrainN.att` real do cliente MU Online.
    pub fn from_client_att_bytes(raw: &[u8]) -> Result<Self, TerrainParseError> {
        let per_tile = match raw.len() {
            65540 => 1,
            131076 => 2,
            n => return Err(TerrainParseError::UnexpectedSize(n)),
        };
        let dec = decrypt_terrain_bytes(raw);
        let hdr = [dec[0], dec[1], dec[2], dec[3]];
        if hdr[0] != 0 || hdr[2] != 255 || hdr[3] != 255 {
            return Err(TerrainParseError::InvalidHeader(hdr));
        }
        let world_num = hdr[1];
        let world = WorldId::from_code(world_num).unwrap_or(WorldId::Lorencia);
        let tiles: Vec<u8> = dec[4..].chunks_exact(per_tile).map(|c| c[0]).collect();
        Ok(Self {
            world,
            raw_world_number: world_num,
            from_client_att: true,
            tiles,
        })
    }

    /// Cria um terreno sintético base para um `WorldId` caso o cliente não esteja presente no disco.
    pub fn synthetic_for(world: WorldId) -> Self {
        let mut tiles = vec![0u8; MAP_SIZE * MAP_SIZE];
        for y in 0..MAP_SIZE {
            for x in 0..MAP_SIZE {
                if !(4..251).contains(&x) || !(4..251).contains(&y) {
                    tiles[y * MAP_SIZE + x] = ATTR_NOGROUND | ATTR_NOMOVE;
                }
            }
        }
        if let Some((x0, y0, x1, y1)) = world.safezone_bounds() {
            for y in y0..=y1 {
                for x in x0..=x1 {
                    if (0..MAP_SIZE as i32).contains(&x) && (0..MAP_SIZE as i32).contains(&y) {
                        tiles[y as usize * MAP_SIZE + x as usize] |= ATTR_SAFEZONE;
                    }
                }
            }
        }
        if world == WorldId::Kanturu {
            for x in 60..=110 {
                tiles[180 * MAP_SIZE + x] |= ATTR_NOMOVE;
            }
            for y in 140..=195 {
                tiles[y * MAP_SIZE + 145] |= ATTR_NOMOVE;
            }
        }
        Self {
            world,
            raw_world_number: world.code(),
            from_client_att: false,
            tiles,
        }
    }

    pub fn from_raw_tiles(world: WorldId, tiles: Vec<u8>) -> Self {
        assert_eq!(tiles.len(), MAP_SIZE * MAP_SIZE);
        Self {
            world,
            raw_world_number: world.code(),
            from_client_att: false,
            tiles,
        }
    }

    pub fn attr(&self, x: i32, y: i32) -> Option<u8> {
        if !(0..MAP_SIZE as i32).contains(&x) || !(0..MAP_SIZE as i32).contains(&y) {
            return None;
        }
        Some(self.tiles[y as usize * MAP_SIZE + x as usize])
    }

    pub fn is_walkable(&self, x: i32, y: i32) -> bool {
        self.attr(x, y)
            .is_some_and(|a| a & (ATTR_NOMOVE | ATTR_NOGROUND) == 0)
    }

    pub fn is_safezone(&self, x: i32, y: i32) -> bool {
        self.attr(x, y).is_some_and(|a| a & ATTR_SAFEZONE != 0)
    }

    pub fn safe_tiles(&self) -> Vec<(i32, i32)> {
        (0..MAP_SIZE * MAP_SIZE)
            .filter(|i| self.tiles[*i] & ATTR_SAFEZONE != 0)
            .map(|i| ((i % MAP_SIZE) as i32, (i / MAP_SIZE) as i32))
            .collect()
    }

    /// Calcula em passos a pé (8 direções, BFS) a distância de cada tile andável do mapa
    /// até qualquer uma das origens em `sources` (ex.: `safe_tiles()` + `arrival_entry_tiles()`).
    /// Tiles inalcançáveis recebem `u16::MAX`.
    pub fn distances_from(&self, sources: &[(i32, i32)]) -> Vec<u16> {
        let mut dist = vec![u16::MAX; MAP_SIZE * MAP_SIZE];
        let mut queue = std::collections::VecDeque::new();
        for &(x, y) in sources {
            if self.attr(x, y).is_some() {
                let idx = y as usize * MAP_SIZE + x as usize;
                if dist[idx] != 0 {
                    dist[idx] = 0;
                    queue.push_back((x, y));
                }
            }
        }
        while let Some((x, y)) = queue.pop_front() {
            let d = dist[y as usize * MAP_SIZE + x as usize];
            for dy in -1..=1 {
                for dx in -1..=1 {
                    let (nx, ny) = (x + dx, y + dy);
                    if (dx, dy) == (0, 0) || !self.is_walkable(nx, ny) {
                        continue;
                    }
                    let i = ny as usize * MAP_SIZE + nx as usize;
                    if dist[i] == u16::MAX {
                        dist[i] = d.saturating_add(1);
                        queue.push_back((nx, ny));
                    }
                }
            }
        }
        dist
    }

    /// Distância a pé (BFS) da entrada/Safe Zone do mapa até o tile `(x, y)`.
    pub fn walking_distance_from_entry(&self, x: i32, y: i32) -> Option<u32> {
        if !(0..MAP_SIZE as i32).contains(&x) || !(0..MAP_SIZE as i32).contains(&y) {
            return None;
        }
        let mut sources = self.safe_tiles();
        sources.extend_from_slice(self.world.arrival_entry_tiles());
        if sources.is_empty() {
            return None;
        }
        let dists = self.distances_from(&sources);
        let d = dists[y as usize * MAP_SIZE + x as usize];
        (d != u16::MAX).then_some(u32::from(d))
    }

    pub fn set_blocked(&mut self, x: i32, y: i32, blocked: bool) {
        if (0..MAP_SIZE as i32).contains(&x) && (0..MAP_SIZE as i32).contains(&y) {
            let idx = y as usize * MAP_SIZE + x as usize;
            if blocked {
                self.tiles[idx] |= ATTR_NOMOVE;
            } else {
                self.tiles[idx] &= !(ATTR_NOMOVE | ATTR_NOGROUND);
            }
        }
    }

    /// Calcula rota A* em 8 direções com heurística octile (sem cortar quinas de paredes).
    pub fn find_path(&self, from: (i32, i32), to: (i32, i32)) -> Option<Vec<(i32, i32)>> {
        self.find_path_internal(from, to, false)
    }

    /// Rota de escape: permite diagonal entre quinas apenas quando `find_path` não tem saída.
    pub fn find_path_escape(&self, from: (i32, i32), to: (i32, i32)) -> Option<Vec<(i32, i32)>> {
        self.find_path_internal(from, to, true)
    }

    fn find_path_internal(
        &self,
        from: (i32, i32),
        to: (i32, i32),
        cut_corners: bool,
    ) -> Option<Vec<(i32, i32)>> {
        use std::cmp::Reverse;
        use std::collections::BinaryHeap;

        if !self.is_walkable(from.0, from.1) || !self.is_walkable(to.0, to.1) {
            return None;
        }
        let idx = |x: i32, y: i32| y as usize * MAP_SIZE + x as usize;
        let heuristic = |x: i32, y: i32| {
            let (dx, dy) = ((x - to.0).abs(), (y - to.1).abs());
            (10 * (dx + dy) - 6 * dx.min(dy)) as u32
        };

        let mut g = vec![u32::MAX; MAP_SIZE * MAP_SIZE];
        let mut came = vec![u32::MAX; MAP_SIZE * MAP_SIZE];
        let mut open = BinaryHeap::new();

        g[idx(from.0, from.1)] = 0;
        open.push(Reverse((heuristic(from.0, from.1), 0u32, from.0, from.1)));

        while let Some(Reverse((_, gc, x, y))) = open.pop() {
            if (x, y) == to {
                let mut path = vec![(x, y)];
                let mut cur = idx(x, y);
                while came[cur] != u32::MAX && came[cur] as usize != idx(from.0, from.1) {
                    cur = came[cur] as usize;
                    path.push(((cur % MAP_SIZE) as i32, (cur / MAP_SIZE) as i32));
                }
                path.reverse();
                return Some(path);
            }
            if gc > g[idx(x, y)] {
                continue;
            }
            for (dx, dy) in [
                (1, 0),
                (-1, 0),
                (0, 1),
                (0, -1),
                (1, 1),
                (1, -1),
                (-1, 1),
                (-1, -1),
            ] {
                let (nx, ny) = (x + dx, y + dy);
                if !self.is_walkable(nx, ny) {
                    continue;
                }
                if !cut_corners
                    && dx != 0
                    && dy != 0
                    && !(self.is_walkable(x + dx, y) && self.is_walkable(x, y + dy))
                {
                    continue;
                }
                let ng = gc + if dx != 0 && dy != 0 { 14 } else { 10 };
                if ng < g[idx(nx, ny)] {
                    g[idx(nx, ny)] = ng;
                    came[idx(nx, ny)] = idx(x, y) as u32;
                    open.push(Reverse((ng + heuristic(nx, ny), ng, nx, ny)));
                }
            }
        }
        None
    }

    pub fn line_clear(&self, a: (i32, i32), b: (i32, i32)) -> bool {
        let n = (b.0 - a.0).abs().max((b.1 - a.1).abs()).max(1);
        (0..=n).all(|k| {
            let x = a.0 as f32 + (b.0 - a.0) as f32 * k as f32 / n as f32;
            let y = a.1 as f32 + (b.1 - a.1) as f32 * k as f32 / n as f32;
            self.is_walkable(x.round() as i32, y.round() as i32)
        })
    }

    pub fn waypoints(
        &self,
        from: (i32, i32),
        path: &[(i32, i32)],
        max_step: i32,
    ) -> Vec<(i32, i32)> {
        let mut out = Vec::new();
        let mut cur = from;
        let mut i = 0;
        while i < path.len() {
            let mut best = i;
            for (j, &p) in path.iter().enumerate().skip(i) {
                if (p.0 - cur.0).abs().max((p.1 - cur.1).abs()) > max_step {
                    break;
                }
                if self.line_clear(cur, p) {
                    best = j;
                }
            }
            out.push(path[best]);
            cur = path[best];
            i = best + 1;
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn astar_avoids_walls_and_forbids_corner_cutting() {
        let mut grid = TerrainGrid::from_raw_tiles(WorldId::Lorencia, vec![0; MAP_SIZE * MAP_SIZE]);
        grid.set_blocked(1, 0, true);
        grid.set_blocked(0, 1, true);
        assert_eq!(grid.find_path((0, 0), (1, 1)), None);

        let kanturu = TerrainGrid::synthetic_for(WorldId::Kanturu);
        let route = kanturu
            .find_path((34, 217), (123, 233))
            .expect("deve achar rota válida em Kanturu");
        assert_eq!(route.last().copied(), Some((123, 233)));
        assert!(route.iter().all(|&(x, y)| kanturu.is_walkable(x, y)));
    }
}
