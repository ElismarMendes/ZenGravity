//! Mapas do MU Online (`WorldId`), grid 256×256 (`Terrain`), zonas seguras e Pathfinding A*.

pub const MAP_SIZE: usize = 256;
pub const ATTR_SAFEZONE: u8 = 0x01;
pub const ATTR_NOMOVE: u8 = 0x04;
pub const ATTR_NOGROUND: u8 = 0x08;

/// Identificadores de mundos (WorldN) validados no cliente MU Online.
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

    /// Retângulo principal aproximado da Safe Zone para exibição no Mapa 2D.
    pub fn safezone_bounds(self) -> Option<(i32, i32, i32, i32)> {
        match self {
            WorldId::Lorencia => Some((130, 118, 152, 138)),
            WorldId::Devias => Some((195, 32, 228, 68)),
            WorldId::Noria => Some((165, 90, 195, 122)),
            WorldId::Kanturu => Some((16, 208, 42, 228)),
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
    tiles: Vec<u8>,
}

impl TerrainGrid {
    /// Cria um terreno sintético base para um `WorldId`, com bordas bloqueadas,
    /// safezone demarcada e obstáculos internos conhecidos para visualização e testes de rota.
    pub fn synthetic_for(world: WorldId) -> Self {
        let mut tiles = vec![0u8; MAP_SIZE * MAP_SIZE];
        // Bordas externas 0..4 e 251..256 sem chão
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
        // Parede decorativa interna para demonstrar desvio de obstáculos A*
        if world == WorldId::Kanturu {
            for x in 60..=110 {
                tiles[180 * MAP_SIZE + x] |= ATTR_NOMOVE;
            }
            for y in 140..=195 {
                tiles[y * MAP_SIZE + 145] |= ATTR_NOMOVE;
            }
        }
        Self { world, tiles }
    }

    pub fn from_raw_tiles(world: WorldId, tiles: Vec<u8>) -> Self {
        assert_eq!(tiles.len(), MAP_SIZE * MAP_SIZE);
        Self { world, tiles }
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
                if dx != 0
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn astar_avoids_walls_and_forbids_corner_cutting() {
        let mut grid = TerrainGrid::from_raw_tiles(WorldId::Lorencia, vec![0; MAP_SIZE * MAP_SIZE]);
        grid.set_blocked(1, 0, true);
        grid.set_blocked(0, 1, true);
        // Quina fechada em (1,0) e (0,1): não pode passar na diagonal (0,0) -> (1,1)
        assert_eq!(grid.find_path((0, 0), (1, 1)), None);

        let kanturu = TerrainGrid::synthetic_for(WorldId::Kanturu);
        let route = kanturu
            .find_path((34, 217), (123, 233))
            .expect("deve achar rota válida em Kanturu");
        assert_eq!(route.last().copied(), Some((123, 233)));
        assert!(route.iter().all(|&(x, y)| kanturu.is_walkable(x, y)));
    }
}
