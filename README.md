# ZenGravity

**ZenGravity** é uma plataforma nativa do Windows para monitoramento em tempo real e coordenação operacional de múltiplos computadores (PCs) e múltiplos personagens de **MU Online**, com operação **Desktop-First** e **Offline-First**.

> **Status Atual:**
> - **Fase 0** (Auditoria e Arquitetura): Concluída.
> - **Fase 1** (Painel Desktop Nativo `zengravity-desktop.exe` + Central Reativa + Simulador Multi-PC): Concluída.
> - **Fase 2** (Leitores Reais do Windows em Modo Somente Leitura + Terrenos Reais `EncTerrain*.att` de `C:\The Classic Games\The Classic MU\Data` + 127 Fixtures Reais): Concluída.
> - **Alternância Ao Vivo no Desktop:** o painel detecta automaticamente janelas `MainMU.exe` abertas na sessão do usuário e permite alternar com 1 clique no topo entre **`🛡 MODO REAL — SOMENTE LEITURA (MainMU.exe)`** e **`⚡ MODO SIMULAÇÃO (4 PCs / 9 Personagens)`**, sem enviar nenhum comando de mouse ou teclado ao jogo.

---

## 1. Principais Características (Fase 1)

- **Painel Desktop 100% Nativo (`zengravity-desktop.exe`):** construído em Rust com `egui`/`eframe` e aceleração gráfica `wgpu`. Abre instantaneamente no Windows sem exigir navegador, Electron, Tauri/WebView2 ou conexão com a internet.
- **Transparência e Confiabilidade de Dados:**
  - Distinção explícita entre `Farmando`, `Parado`, `Recuperando`, `Morto`, `Desconectado (DC)`, `Pausado`, `Online`, `Desconhecido` e `Leitura Desatualizada (Stale)`.
  - Cada métrica (Zen, HP, Posição, Helper, Dark Raven, Objetos Próximos) registra sua **Origem** (`RAM Travada`, `OCR Recortado`, `Modelo de Pixel`, `Simulador`, `Desconhecida`), **Idade da Leitura** e **Confiabilidade**.
  - **Honestidade de Observação:** áreas fora do raio de visão (`12 tiles`) de um personagem online nunca são exibidas como "vazias" ou "livres", e sim como **Não Observadas**.
- **Simulador Reproduzível Multi-PC (`zengravity-simulator`):**
  - Simula **4 PCs** (`PC1-PRINCIPAL`, `PC2-FARM-NORTE`, `PC3-ARENA-SUL`, `ELISMAR-NOTE`) e **9 personagens** (`x9901`, `ReDMooN`, `AMUMU`, `LOBOLIXO`, `CSRF`, `0MAGIC`, `xWizard`, `YouSee`, `FirstHand`).
  - Emite eventos reais pelo protocolo (`zengravity-protocol`) passando pela Central (`zengravity-central`): ganho de Zen/h, previsão de 2 bilhões (`ETA 2B`), variação de HP e Dark Raven, caminhada A*, disputa de spots, aparição de Golden Monsters em Lorencia, eventos de PK, queda de conexão (DC) e simulação de leitura velha (`Stale`).
- **Mapa 2D Tático Interativo:**
  - Zoom com roda do mouse, pan por arraste, grid `0..=255`, zonas seguras (`Safezone`), paredes/obstáculos, spots catalogados (`F1`/`F2`), raio de cobertura real de cada personagem, jogadores, monstros, NPCs e Golden Monsters com idade da detecção.
- **Segurança Inegociável desde o Núcleo:**
  - Lista fechada de comandos tipados (`Command`), botão global de **Parada de Emergência**, agendador de sessão única de entrada física por PC (`InputSessionScheduler`) e persistência local em SQLite (modo WAL).

---

## 2. Estrutura do Workspace

```text
ZenGravity/
├── Cargo.toml                  # Configuração do workspace Rust
├── .env.example                # Exemplo de variáveis de ambiente
├── docs/
│   ├── VISAO.md                # Visão do produto e princípios de segurança
│   ├── ARQUITETURA.md          # Arquitetura modular, fluxo de dados e estados
│   ├── MIGRACAO_LEGADO.md      # Auditoria completa do ZenMonitor e matriz de migração
│   ├── ROADMAP.md              # Planejamento incremental por fases
│   ├── CRITERIOS_DE_ACEITE.md  # Critérios verificáveis das Fases 0 e 1
│   └── DECISOES/
│       └── 0001-stack-desktop.md # ADR 0001: Escolha de Rust + egui/eframe + wgpu
└── crates/
    ├── core/                   # zengravity-core: domínio puro (0 dependências de SO/rede)
    ├── protocol/               # zengravity-protocol: contratos versionados, snapshots e deltas
    ├── central/                # zengravity-central: Hub multi-PC, SQLite WAL e servidor WS local
    ├── agent/                  # zengravity-agent: agendador de sessão única, guarda F8 e cliente
    ├── simulator/              # zengravity-simulator: simulador determinístico multi-PC (4 PCs, 9 chars)
    ├── desktop/                # zengravity-desktop: aplicativo nativo Windows (egui/eframe + wgpu)
    └── test-support/           # zengravity-test-support: relógio determinístico e fixtures de teste
```

---

## 3. Como Compilar e Executar no Windows

### Pré-requisitos
- Windows 10 ou Windows 11 (64-bit)
- Rust Toolchain estável (`rustc >= 1.85`, instalado via `rustup`)

### 3.1 Executar o Painel Desktop Nativo (com Central + Simulador integrados)
Por padrão, o `zengravity-desktop` sobe automaticamente a Central local em background e conecta o Simulador Multi-PC via protocolo real:

```powershell
cargo run -p zengravity-desktop --release
```
*(Ou em modo debug durante o desenvolvimento: `cargo run -p zengravity-desktop`)*

### 3.2 Executar Central + Simulador em Terminal Separado e Conectar via WebSocket
Se desejar testar a operação em processos separados via rede local (`ws://127.0.0.1:8790/ws/panel`):

1. **Terminal 1 — Iniciar a Central com Simulador:**
   ```powershell
   cargo run -p zengravity-central -- --bind 127.0.0.1:8790 --with-simulator
   ```
2. **Terminal 2 — Abrir o Desktop conectado via WebSocket à Central:**
   ```powershell
   cargo run -p zengravity-desktop -- --connect ws://127.0.0.1:8790/ws/panel
   ```

### 3.3 Executar a Suíte Completa de Testes e Verificação de Qualidade
```powershell
# Rodar todos os testes unitários e de integração do workspace
cargo test --workspace

# Verificar formatação do código
cargo fmt --all -- --check

# Verificar lints rigorosos do Clippy
cargo clippy --workspace --all-targets -- -D warnings
```
