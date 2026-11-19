# Critérios de Aceite — ZenGravity (Fase 0 e Fase 1)

Este documento estabelece os critérios objetivos de validação para considerar as Fases 0 e 1 concluídas com qualidade de engenharia e aderência total às diretrizes do proprietário.

---

## 1. Critérios da Fase 0 (Auditoria, Segurança e Projeto)

| ID | Critério | Como Verificar | Status |
|---|---|---|---|
| **F0-01** | Repositório legado `ZenMonitor` preservado sem nenhuma alteração, commit ou interferência. | `git remote -v` aponta exclusivamente para `ZenGravity.git`; nenhum push ou alteração feita no `ZenMonitor`. | Aprovado |
| **F0-02** | Matriz de auditoria do legado documentando código real, divergências (doc vs código vs testes), decisão de migração, risco e teste necessário. | Leitura de [`docs/MIGRACAO_LEGADO.md`](MIGRACAO_LEGADO.md). | Aprovado |
| **F0-03** | Arquitetura modular documentada separando `core`, `protocol`, `central`, `agent`, `simulator`, `desktop` e `test-support`. | Leitura de [`docs/ARQUITETURA.md`](ARQUITETURA.md) e [`Cargo.toml`](../Cargo.toml). | Aprovado |
| **F0-04** | Decisão de stack desktop nativa (`egui`/`eframe` + `wgpu`) registrada em ADR com alternativas rejeitadas e verificação de licenças. | Leitura de [`docs/DECISOES/0001-stack-desktop.md`](DECISOES/0001-stack-desktop.md). | Aprovado |
| **F0-05** | Documentação completa exigida: `README.md`, `docs/VISAO.md`, `docs/ARQUITETURA.md`, `docs/ROADMAP.md`, `docs/MIGRACAO_LEGADO.md`, `docs/DECISOES/0001-stack-desktop.md`, `docs/CRITERIOS_DE_ACEITE.md`. | Conferência de arquivos na árvore do repositório. | Aprovado |

---

## 2. Critérios da Fase 1 (Workspace, Simulador, Central e Desktop Executável)

| ID | Critério | Como Verificar | Status |
|---|---|---|---|
| **F1-01** | Workspace Rust compila sem erros no Windows (`cargo build --workspace`) e passa em todos os testes (`cargo test --workspace`) e lints (`cargo clippy --workspace --all-targets -- -D warnings`). | Execução dos comandos Cargo no terminal Windows. | Aprovado |
| **F1-02** | `.gitignore` seguro (bloqueando `.env`, `*.db`, `*.key`, `Settings.ini`, `target/`) e workflow de GitHub Actions configurado. | Conferência de [`.gitignore`](../.gitignore) e [`.github/workflows/ci.yml`](../.github/workflows/ci.yml). | Aprovado |
| **F1-03** | Distinção rigorosa de estados verdadeiros (`Online`, `Desconectado`, `Morto`, `Recuperando`, `Farmando`, `Parado`, `Pausado`, `Desconhecido`, `Leitura Desatualizada / Stale`) com registro de origem, momento e confiabilidade em cada métrica. | Testes unitários em `zengravity-core` e inspeção visual nos cards e ficha de detalhes do `zengravity-desktop`. | Aprovado |
| **F1-04** | Simulador reproduzível (`zengravity-simulator`) com pelo menos 3 PCs (4 PCs e 9 personagens configurados por padrão) emitindo variações reais de Zen, HP, Dark Raven, posição, Helper, mortes/PK, eventos e quedas/atrasos de conexão. | Execução do simulador e testes automatizados de determinismo por semente (`seed`). | Aprovado |
| **F1-05** | Eventos do simulador trafegam pelo protocolo real (`zengravity-protocol`) através do Hub da Central (`zengravity-central`) até o aplicativo desktop (`zengravity-desktop`), nunca como números aleatórios soltos na tela. | Teste de integração fim-a-fim (`simulator -> central -> protocol delta -> client state`) e execução do binário. | Aprovado |
| **F1-06** | `zengravity-desktop.exe` abre janela nativa no Windows com tema escuro profissional, badge visível **`MODO SIMULAÇÃO`**, barra lateral, cabeçalho com status da central, cards por PC/char, lista filtrável, painel de detalhes com sparkline, seção de eventos, zona de alertas e tela de Mapa 2D. | Execução de `cargo run -p zengravity-desktop` no Windows. | Aprovado |
| **F1-07** | Tela de Mapa 2D permite zoom e pan, mostra grid/coordenadas `0..=255`, safezones, spots catalogados, personagens e entidades próximas com seu **raio real de observação** (nunca fingindo leitura do mapa inteiro fora do alcance dos personagens). | Aba `Mapa 2D & Spots` no `zengravity-desktop.exe`. | Aprovado |
| **F1-08** | Tratamento de leitura desatualizada (`stale`) e reconexão: quando um agente atrasa ou a central pausa o envio, o desktop marca os dados como `LEITURA DESATUALIZADA (STALE)` sem congelar a interface e ressincroniza quando a comunicação retorna. | Botão `"Simular Atraso / Stale"` na barra do simulador do desktop e testes automatizados de `stale`/reconexão. | Aprovado |
| **F1-09** | Honestidade sobre o escopo: nenhuma afirmação de que o sistema já foi testado contra o cliente real do MU Online nesta Fase 1. | Documentado no `README.md` e no relatório de entrega. | Aprovado |
