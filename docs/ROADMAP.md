# Roadmap do Projeto — ZenGravity

O desenvolvimento do **ZenGravity** é organizado em fases incrementais e verificáveis. Nenhuma fase avança para produção sem validação explícita do proprietário.

---

## Fase 0 — Auditoria do Legado e Arquitetura Base (Concluída nesta entrega)
- [x] Auditoria completa do repositório legado `ZenMonitor` (código, testes, banco, protocolo e 17 ADRs).
- [x] Mapeamento de divergências entre documentação legada e código real (`docs/MIGRACAO_LEGADO.md`).
- [x] Definição da arquitetura modular (`core`, `protocol`, `central`, `agent`, `simulator`, `desktop`, `test-support`) em `docs/ARQUITETURA.md`.
- [x] Registro de decisão arquitetural da stack desktop nativa `egui`/`eframe` + `wgpu` (`docs/DECISOES/0001-stack-desktop.md`).
- [x] Documentação de visão do produto (`docs/VISAO.md`) e critérios de aceite (`docs/CRITERIOS_DE_ACEITE.md`).

## Fase 1 — Primeira Versão Executável, Painel Desktop Nativo e Simulador (Concluída nesta entrega)
- [x] Workspace Rust configurado com lints (`clippy`), formatação (`rustfmt`), `.gitignore` seguro e workflow de CI (`.github/workflows/ci.yml`).
- [x] Domínio puro em `zengravity-core` com `ZenTracker` (`Zen/h`, `ETA 2B`), máquina de alertas, ciclo de vida de personagens (`CharLifecycleState`), mapas (`Terrain`, A*), diretório de spots e parsers.
- [x] Protocolo tipado e versionado em `zengravity-protocol` com metadados de leitura (`MetricReading<T>`, origem, idade, confiança), snapshots, deltas sequenciados, ACK de comandos e estado `stale`.
- [x] Central reativa em `zengravity-central` com Hub multi-PC, persistência SQLite (WAL) e servidor WebSocket local.
- [x] Simulador reproduzível em `zengravity-simulator` com **4 PCs e 9 personagens** simulando ganho de Zen, variações de HP, Dark Raven, movimentação no mapa 2D, disputas de spot, Golden Monsters em Lorencia, mortes/PK, desconexão (DC) e atraso de leitura (`stale`).
- [x] Aplicativo nativo **`zengravity-desktop.exe`** com:
  - Indicador visível de **`MODO SIMULAÇÃO`**;
  - Barra lateral, cabeçalho com saúde da central e botão de **Parada de Emergência**;
  - Cards por PC/personagem e lista filtrável por PC, estado, mapa e busca textual;
  - Painel lateral de detalhes do personagem com gráfico temporal (sparkline de Zen), diagnósticos de leitura e ações;
  - Seção de eventos em tempo real (PK, Golden, avisos, comandos) e zona de alertas ativos;
  - Tela de **Mapa 2D interativo** (zoom, pan, grid 256×256, safezones, raio de observação real, spots catalogados, jogadores, mobs e goldens sem inventar cobertura integral do mapa);
  - Detecção automática de leitura desatualizada (`stale`) e reconexão automática sem congelar a interface.

---

## Fase 2 — Leitores Reais do Windows (Somente Leitura e Diagnóstico Seguro) (Concluída nesta entrega)
- [x] Enumeração segura de janelas e processos do MU Online no Windows (`HWND`, `PID`, tempo de início do processo, caminho do executável e parser de título EN/PT).
- [x] Captura única por ciclo via `PrintWindow` (`FrameImage`) com validação de integridade do quadro (rejeição de janelas minimizadas ou redimensionadas no meio da captura) e recortes em memória.
- [x] Leitura de mapa, coordenadas `(x, y)`, ícone do Helper, campo do Zen, número de HP e barra do Dark Raven por modelos de pixel calibrados com **127 fixtures reais do cliente**.
- [x] Máquina de trava de RAM somente leitura (`ReadProcessMemory`, apenas `PROCESS_VM_READ`) para Zen (`u32`) e HP (`u16`) com persistência de travas por `(pid, process_start_time, char_name)` em `data/ram_locks.json`.
- [x] Leitura da lista de objetos próximos (`0x598` stride: jogadores, monstros, NPCs e Golden Monsters) e verificação de conexão TCP, exibindo tudo ao vivo no `zengravity-desktop.exe` com **`🛡 MODO REAL — SOMENTE LEITURA (MainMU.exe)`** e carregamento direto dos terrenos `Data\WorldN\EncTerrainN.att` de `C:\The Classic Games\The Classic MU\Data`.

## Fase 3 — Catálogo Coordenado de Spots e Atalhos F1/F2 no Jogo
*Próxima fase recomendada (não executada ainda).*
- [ ] Captura física dos atalhos `F1` (spot principal) e `F2` (catálogo) com janela em foco.
- [ ] Persistência e sincronização multi-PC do diretório de spots, reservas (`leases`), `friendly_chars` e visualização de cobertura/ocupação real no Mapa 2D do desktop.

## Fase 4 — Agendador de Entrada Física e Navegação Supervisionada
- [ ] Implementação Win32 do `InputSessionScheduler` (1 rotina por PC por vez, guarda contra pânico, verificação de foco/PID antes de cada passo, gancho `F8` de parada imediata e badge discreto).
- [ ] Caminhada supervisionada por A* com vigia de progresso (`NavTuning`), desvio local de mobs e retorno automático ao spot após morte ou DC.
- [ ] Busca manual (`"Procurar spot agora"` com seletor de mapa) e fallback configurável (Mapa Original → Arena → Alternativos, com reavaliação periódica sem sobrescrever o `F1`).

## Fase 5 — Rotinas Especializadas e Operação Avançada
- [ ] Depósito automático no baú, Fortuna do Imperador (Lugard), restauração do Dark Raven no Trainer de Lorencia (verificando custo em Zen) e patrulha/roteamento de Golden Monsters baseado em evidência real na memória.
- [ ] Mini-painel flutuante opcional (overlay sempre no topo do desktop), notificações locais/celular (`ntfy`), backup automatizado e atualizações assinadas (`Ed25519`) com canal de teste e rollback.
