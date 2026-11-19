# Auditoria e Matriz de Migração do Legado (`ZenMonitor` → `ZenGravity`)

Este documento registra a auditoria técnica completa realizada sobre o repositório legado **`ZenMonitor`** (em sua revisão `v5-modular` / pós-`estavel-138`, contendo 5 crates, ~19–25 mil linhas de Rust e 17 ADRs em `docs/decisoes/`), apontando as discrepâncias encontradas entre documentação, código e testes, e definindo o plano consciente de reaproveitamento conceitual, reescrita ou descarte para o **ZenGravity**.

---

## 1. Divergências Encontradas entre Documentação, Código e Testes no Legado

Durante a leitura direta do código-fonte do `ZenMonitor`, identificamos diferenças importantes entre o que dizem os documentos (`docs/ARQUITETURA.md`, `docs/V5.md`, `docs/ROTEIRO.md` e `docs/decisoes/0017-*.md`) e o que está realmente no código:

1. **Representação de Spots e Catálogo no Protocolo (`protocol.rs` vs `ARQUITETURA.md` / `0017`):**
   - `docs/ARQUITETURA.md` (item 5 da seção 2.1) aponta como erro grave ter o spot espalhado em vários campos e recomenda um tipo único `Spot { world, x, y }`.
   - Embora `zen-core/src/spots.rs` tenha introduzido `Spot { world, x, y }`, o contrato `CharReport` e `PanelChar` em `crates/zen-core/src/protocol.rs` manteve campos legados duplicados lado a lado: `home_spot: Option<Spot>`, `spot: Option<(i32, i32)>`, `spot_map: Option<String>`, além de `AgentMsg::Report` separar `catalog: Vec<(i32, i32)>` (exclusivo para Kanturu 1) e `catalog_other: Vec<(u8, i32, i32)>`.
   - **No ZenGravity:** unificamos desde o dia 1 em `SpotCoord { world: WorldId, x: u8, y: u8 }` em todos os relatórios e catálogos.

2. **Mensagens de Status em Texto Livre vs Estados Tipados (`protocol.rs` / `ciclo.rs`):**
   - `docs/ARQUITETURA.md` (item 10 da seção 2.2) critica que `auto_status`, `lock_status` e `hp_status` são `String` livres e que o painel faz `match` por substring. Mesmo na `v5`, `CharReport` e `ManualSearch::state` continuaram usando `String` (`"pendente"`, `"procurando"`, `"spot (160,197)"`).
   - **No ZenGravity:** o estado operacional do personagem (`CharLifecycleState`), a origem e confiabilidade de cada métrica (`MetricReading<T>`, `MetricSource`, `Confidence`) e o estado de busca de spot são `enum`s estritamente tipados no protocolo.

3. **Distância de Espaçamento e Chegada (`spots.rs` vs `0017-catalogo-fallback-spots.md`):**
   - O ADR `0017` menciona originalmente margem de 3 tiles, depois corrige no final para `ARRIVAL_RADIUS = 2`, e menciona reserva a até 2 tiles e jogador externo a 6 tiles.
   - Porém, em `crates/zen-core/src/spots.rs` (linhas 5–11), os valores reais foram alterados em 18/11 para `ARRIVAL_RADIUS = 2`, `OWN_SPACING = 8` (para evitar KS entre dois personagens próprios cujo Helper tem alcance 6) e `OCCUPIED_RADIUS = 12` (dois personagens a até 12 tiles disputam mobs).
   - **No ZenGravity:** preservamos as constantes reais validadas em campo (`ARRIVAL_RADIUS = 2`, `OWN_SPACING = 8`, `OCCUPIED_RADIUS = 12`) e parametrizamos na configuração de domínio.

4. **Recuperação Acoplada a "Kanturu" (`recovery.rs`):**
   - `crates/zen-core/src/recovery.rs` ainda usa `in_kanturu: Option<bool>` e `Trigger::OutsideKanturu` na máquina de estados pura, apesar de o sistema já operar em múltiplos mapas (`Arena`, `Dungeon`, `Lost Tower`, `Aida`, etc.).
   - **No ZenGravity:** a máquina de recuperação é agnóstica de mapa (`OutsideTargetWorld`), comparando o `WorldId` observado com o `SpotCoord` alvo (principal ou alternativo ativo).

5. **Relógio de Parede vs Tempo Monotônico (`recovery.rs`, `hub.rs`):**
   - O legado sofreu paradas reais em produção (ex.: `DESKTOP-E1GLBEF` com relógio atrasado 5h deixou o personagem 46 min parado ou com falso alerta `"PAROU"`). Mitigações pontuais (`if self.wait_until > now + 86_400`, cálculo de `skew` no `hub.rs`) foram adicionadas, mas o núcleo ainda mistura timestamp Unix com duração monotônica.
   - **No ZenGravity:** separamos `MonotonicMs` (para timeouts, cooldowns, janelas de progresso e detecção de `stale`) de `UtcTimestampSecs` (exclusivo para auditoria, banco de dados e exibição).

6. **Envio de Estado Inteiro no WebSocket do Painel (`central/src/main.rs` vs `V5.md`):**
   - Na `v5`, o painel web passou a fazer diff incremental no DOM (`morph`), mas o canal `/ws/panel` ainda serializa snapshots completos com frequência.
   - **No ZenGravity:** o protocolo entre Central e Desktop envia um `Snapshot` inicial (com `seq`) e depois transmite `DeltaEvent`s sequenciados (`AgentReportApplied`, `CharStateChanged`, `AlertRaised`, `AlertCleared`, `EventLogged`, `Heartbeat`), permitindo ressincronização por snapshot caso haja salto de `seq` ou reconexão.

---

## 2. Matriz de Migração (`Legado → ZenGravity`)

| Funcionalidade do Legado | Evidência no Código Legado | Decisão (`Aproveitar Ideia` / `Reescrever` / `Descartar`) | Risco | Teste Necessário no ZenGravity |
|---|---|---|---|---|
| **Cálculo de Zen/h, filtro de salto de baú (`MAX_FARM_STEP = 30M`) e ETA 2B** | `crates/zen-core/src/rate.rs` (`ZenTracker`, `eta_to_limit`) | **Aproveitar ideia e reescrever limpo** em `zengravity-core::telemetry` com suporte a metadados de fonte/idade | Baixo | Testes unitários de ritmo constante, salto de baú (>30M ignorado como farm), queda por depósito, leituras fora de ordem e cálculo de ETA para 2B. |
| **Avaliação de Alertas e Prioridade (`Offline` > `Dc` > `Limit2B` > `Stopped` > `Dead` > `Near2B`)** | `crates/zen-core/src/alerts.rs` (`evaluate`, `NotifyGate`) | **Aproveitar ideia e expandir** em `zengravity-core::alerts` incluindo estado explícito `Stale` (leitura desatualizada) e `Paused` | Baixo | Testes unitários garantindo que `Offline`/`Stale` suprimem falsos alertas derivados (como `Stopped`) e que `NotifyGate` respeita cooldown. |
| **Trava de RAM de Zen (`u32`) e HP (`u16`) guiada por semente de tela + `DeathDetector`** | `crates/zen-core/src/ramlock.rs`, `crates/zen-core/src/hplock.rs`, `crates/agent/src/monitor.rs` | **Aproveitar ideia (algoritmo puro)** para Fase futura de leitura do jogo; modelar estados de trava (`Locked`, `Filtering`, `NeedSeed`) já na telemetria | Médio (defasagem quadro/RAM) | Testes de máquina de estados com defasagem de 1 quadro e descarte apenas após falhas consecutivas. |
| **Parser de Título da Janela (`Name`/`Nome`, `Level`, `Resets`, `WC`, `WP`, `GP`)** | `crates/zen-core/src/title.rs` (`TitleInfo::parse`) | **Aproveitar ideia e reescrever** em `zengravity-core::game_title` suportando cliente EN/PT e sanitização contra caracteres de controle | Baixo | Testes unitários com títulos reais (`ReDMooN`, `xWizard`, `0MAGIC`), tela de login sem char e entradas malformadas. |
| **Parser de Chat (`[PK]` e `[PickItem]`) e Causa da Morte (`Player` vs `Monster`)** | `crates/zen-core/src/chat.rs`, `crates/zen-core/src/pk.rs` (`PkDeduper`) | **Aproveitar ideia** para eventos de PK/chat e deduplicação temporal entre múltiplos PCs observadores | Baixo | Testes unitários de PK por jogador, monstro de 1 palavra (`Genocider`, `Balrog`), monstro com nome truncado (`Golden Saty`) e deduplicação. |
| **Descriptografia de Terreno (`EncTerrainN.att`), Safezone e Pathfinding A\*** | `crates/zen-core/src/terrain.rs` (`Terrain`, `path`, `path_escape`, `distances_from`) | **Aproveitar algoritmo validado** em `zengravity-core::map` (grid 256×256, `SAFEZONE`, `NOMOVE`, `NOGROUND`, A* octile sem cortar quina) | Baixo | Testes de rota contornando parede, proibição de diagonal em quina dupla, cálculo de distância BFS a partir da safezone e waypoints. |
| **Catálogo de Spots (`F1`/`F2`), Espaçamento (`OWN_SPACING=8`, `OCCUPIED_RADIUS=12`), Leases e Fallback** | `crates/zen-core/src/spots.rs`, `docs/decisoes/0017-catalogo-fallback-spots.md` | **Reescrever com modelo unificado** (`SpotCoord { world, x, y }`) preservando regras de posse, reserva com expiração, `friendly_chars` e `ausência de leitura != spot livre` | Médio (concorrência multi-PC) | Testes de reserva exclusiva concorrente, expiração de lease, espaçamento mínimo de 8 tiles entre chars próprios e recusa quando leitura de objetos é `None`. |
| **Lista de Objetos na Memória (`STRIDE = 0x598`) e Detecção de Golden vs Golden Boss** | `crates/zen-core/src/objects.rs` (`Obj`, `is_golden`, `is_golden_boss`) | **Aproveitar modelo de entidades** em `zengravity-core::entities` distinguindo `Player`, `Monster`, `Npc`, `GoldenMonster` e `GoldenBoss` (ex.: `Great Golden Dragon` classe 501), sempre acompanhados de idade/confiança | Médio (offsets dependem da versão do cliente) | Testes unitários de classificação de entidades, distinção entre `Golden Budge Dragon` (alvo) e `Great Golden Dragon` (chefe letal), e expiração visual de leitura antiga. |
| **Protocolo Agente ↔ Central ↔ Painel e Lista Fechada de Comandos** | `crates/zen-core/src/protocol.rs`, `crates/zen-core/src/actions.rs` | **Reescrever em crate dedicado (`zengravity-protocol`)** com versionamento, sequência (`seq`), `Snapshot` + `Delta`, ACK de comandos, origem/confiabilidade de métricas e zero campos legados duplicados | Baixo | Testes de serialização/deserialização, compatibilidade de versão, idempotência de sequência e detecção de gap para ressincronização. |
| **Hub da Central e Persistência SQLite (`zen_points`, `pk_events`, `deaths`, `audit`)** | `crates/central/src/hub.rs`, `crates/central/src/db.rs`, `crates/central/src/main.rs` | **Reescrever em `zengravity-central`** desacoplado do HTML legado, expondo canais em memória e WebSocket local para Agentes e para o `zengravity-desktop` | Baixo | Testes de integração do Hub com múltiplos agentes simulados, detecção de `stale`/desconexão, persistência SQLite e broadcast de deltas. |
| **Painel Web em HTML/JS puro (`crates/central/web/index.html`)** | `crates/central/web/` | **Descartar como interface primária** e substituir pelo aplicativo nativo **`zengravity-desktop`** (`egui`/`eframe` + `wgpu`) | Baixo | Compilação nativa Windows, validação visual com simulador de 3+ PCs, teste de responsividade, reconexão e estado `stale`. |
| **Ferramenta `diag` monolítica (2.200 linhas duplicando código do agente) e scripts da ponte `.ps1`** | `crates/diag/src/main.rs`, `zen.ps1` | **Descartar estrutura monolítica**; substituir por `zengravity-simulator` reproduzível e `zengravity-test-support` | Baixo | Execução determinística/reproduzível do simulador alimentando a central e o desktop via protocolo real. |
| **Automação Windows (`SendInput`, `PrintWindow`, `ReadProcessMemory`, `nav.rs`)** | `crates/agent/src/win/**` | **Não implementar na Fase 1 (manter isolado para Fase 2+)**; na Fase 1, definir os traits de segurança, agendador de sessão única de input por PC e parada de emergência no `zengravity-agent` | Alto se feito sem o jogo instrumentado | Testes de contrato do agendador de input (exclusão mútua por PC e parada imediata por emergência/F8). |
