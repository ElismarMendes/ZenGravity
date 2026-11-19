//! Carregador somente-leitura dos arquivos de terreno do cliente (`Data\WorldN\EncTerrainN.att`).
//!
//! Regra inegociável de segurança: este módulo **apenas** lê arquivos `EncTerrain*.att` dentro
//! de `Data\World*`. Jamais abre ou lê `Settings.ini`, `xAccounts.ini` ou qualquer arquivo
//! de configuração/credenciais do cliente.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use zengravity_core::map::{TerrainGrid, WorldId};

/// Diretórios padrão onde o cliente "The Classic MU" costuma estar instalado no Windows.
pub const DEFAULT_CLIENT_DATA_DIRS: &[&str] = &[
    r"C:\The Classic Games\The Classic MU\Data",
    r"C:\The Classic MU\Data",
];

#[derive(Debug, Clone, Default)]
pub struct ClientTerrainLoader {
    data_dir: Option<PathBuf>,
    cache: HashMap<WorldId, TerrainGrid>,
}

impl ClientTerrainLoader {
    /// Descobre automaticamente a pasta `Data` do cliente a partir de caminhos padrão
    /// ou do caminho do executável `MainMU.exe` em execução.
    pub fn discover(custom_data_dir: Option<&Path>) -> Self {
        let mut chosen = custom_data_dir
            .filter(|p| p.is_dir())
            .map(Path::to_path_buf);

        if chosen.is_none() {
            for candidate in DEFAULT_CLIENT_DATA_DIRS {
                let p = Path::new(candidate);
                if p.is_dir() {
                    chosen = Some(p.to_path_buf());
                    break;
                }
            }
        }
        Self {
            data_dir: chosen,
            cache: HashMap::new(),
        }
    }

    pub fn data_dir(&self) -> Option<&Path> {
        self.data_dir.as_deref()
    }

    pub fn set_data_dir_from_exe_path(&mut self, exe_path: &str) {
        if self.data_dir.is_some() {
            return;
        }
        if let Some(parent) = Path::new(exe_path).parent() {
            let data = parent.join("Data");
            if data.is_dir() {
                self.data_dir = Some(data);
            }
        }
    }

    /// Retorna o `TerrainGrid` de um `WorldId`, lendo e descriptografando `EncTerrainN.att`
    /// do disco na primeira solicitação e mantendo em cache. Se o arquivo não existir,
    /// faz fallback transparente para `TerrainGrid::synthetic_for(world)`.
    pub fn get_or_load(&mut self, world: WorldId) -> &TerrainGrid {
        self.cache.entry(world).or_insert_with(|| {
            if let Some(base) = &self.data_dir {
                let full_path = base.join(world.client_terrain_relative_path());
                // Garantia extra de segurança: só lê arquivos com extensão .att
                if full_path.extension().and_then(|e| e.to_str()) == Some("att") {
                    if let Ok(bytes) = std::fs::read(&full_path) {
                        if let Ok(mut grid) = TerrainGrid::from_client_att_bytes(&bytes) {
                            grid.world = world;
                            return grid;
                        }
                    }
                }
            }
            TerrainGrid::synthetic_for(world)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_real_client_terrain_when_installed_or_falls_back_cleanly() {
        let mut loader = ClientTerrainLoader::discover(None);
        let kanturu = loader.get_or_load(WorldId::Kanturu);
        assert_eq!(kanturu.world, WorldId::Kanturu);
        if Path::new(r"C:\The Classic Games\The Classic MU\Data\World38\EncTerrain38.att").exists()
        {
            assert!(
                kanturu.from_client_att,
                "deve carregar EncTerrain38.att real de C:\\The Classic Games"
            );
            assert!(kanturu.is_walkable(123, 233));
        }
    }
}
