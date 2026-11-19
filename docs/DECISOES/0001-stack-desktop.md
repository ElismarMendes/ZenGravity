# ADR 0001 — Stack Tecnológica e Interface Desktop Nativa

- **Data:** 18/11/2026
- **Status:** Aceito
- **Contexto:** Inicialização do projeto ZenGravity (Fase 0 e Fase 1)

## 1. Contexto e Problema

No sistema legado (`ZenMonitor`), a interface de operação era um arquivo HTML/JS servido pela central via HTTP/WebSocket. Embora tenha funcionado sem etapa de build, essa abordagem exigia abrir um navegador, lidava com limitações de atualização de DOM (perda de foco/hover quando o estado inteiro de ~50 KB era reenviado a cada segundo) e não oferecia a experiência de um aplicativo desktop nativo do Windows com renderização fluida de mapa 2D e operação imediata offline.

O proprietário definiu como requisito central do **ZenGravity**:
- Operar primeiro **localmente, sem internet e sem navegador**, como aplicativo nativo do Windows (`zengravity-desktop.exe`);
- Manter separação limpa entre núcleo de domínio (`core`), contratos (`protocol`), central (`central`), agente (`agent`), simulador (`simulator`) e interface (`desktop`);
- Evitar Electron, Tauri/WebView ou servidor web obrigatório para abrir o painel.

## 2. Decisão

Adotamos a seguinte stack tecnológica em **Rust (Edition 2021)**, com dependências fixadas via `Cargo.lock`:

1. **Linguagem e Workspace:** Rust modular (`zengravity-core`, `zengravity-protocol`, `zengravity-central`, `zengravity-agent`, `zengravity-simulator`, `zengravity-desktop`, `zengravity-test-support`).
2. **Interface Desktop Nativa:** `egui` + `eframe` (`0.31`) com backend de renderização acelerado por GPU via **`wgpu`**.
   - Permite criar grids flexíveis, cards customizados, gráficos de séries temporais (sparklines) e um canvas 2D vetorial nativo (zoom, pan, coordenadas de tiles 256×256, raio de visão, rotas e entidades) em um único executável leve (`.exe`), sem WebView2 nem Chromium.
3. **Runtime Assíncrono e Rede Local:** `tokio` (`1.x`) + `axum` (`0.8`) + `tokio-tungstenite` (`0.26`).
   - O desktop pode operar tanto conectando-se via WebSocket local (`ws://127.0.0.1:8790/ws/panel`) a uma central independente quanto iniciando o pipeline Central + Simulador embutido em processo para testes e demonstração com um único clique/comando.
4. **Serialização e Contratos:** `serde` + `serde_json` (`1.x`) com versionamento explícito de protocolo (`PROTOCOL_VERSION = 1`), números de sequência monotônicos (`seq`), snapshots completos na conexão/reconexão e eventos/deltas incrementais.
5. **Persistência Local:** `sqlx` (`0.8`) com **SQLite** em modo WAL (`Write-Ahead Logging`), garantindo histórico local de telemetria, mortes e auditoria em arquivo único com migrações versionadas.
6. **Observabilidade e Erros:** `tracing` + `tracing-subscriber` para logs estruturados e `thiserror` / `anyhow` para tratamento de erros tipados.

## 3. Alternativas Consideradas e Rejeitadas

| Alternativa | Motivo da Rejeição |
|---|---|
| **Electron / Node.js** | Alto consumo de RAM (>150 MB só para abrir a janela), empacotamento pesado com Chromium e maior latência de atualização em máquinas que já rodam múltiplos clientes de MU Online. |
| **Tauri / WebView2** | Embora mais leve que Electron, ainda depende do runtime WebView2 do navegador instalado no Windows e exige pipeline duplo de build (Rust + Node/bundler frontend) logo na Fase 1. |
| **Painel apenas em HTML servido localmente** | Exige navegador aberto, não oferece janela desktop autônoma nativa e já apresentou gargalos de atualização de DOM e foco no legado. |
| **iced / Slint** | Boas bibliotecas em Rust, porém `egui`/`eframe` com `wgpu` oferece `Painter` imediato muito mais direto e produtivo para desenhar o Mapa 2D tático (grid 256×256, raios de ocupação, zoom/pan livre e gráficos em tempo real) sem boilerplate excessivo. |
| **PostgreSQL** | Complexidade operacional desnecessária para operação local offline; SQLite em modo WAL atende com folga o volume de 5 a 20 PCs locais e permite backup atômico em arquivo único. |

## 4. Licenças e Manutenção Verificadas

Todas as dependências escolhidas possuem manutenção ativa e licenças permissivas compatíveis com software proprietário:
- `egui` / `eframe` / `wgpu`: `MIT OR Apache-2.0`
- `tokio` / `axum` / `tokio-tungstenite`: `MIT`
- `serde` / `serde_json` / `toml`: `MIT OR Apache-2.0`
- `sqlx`: `MIT OR Apache-2.0`
- `tracing` / `thiserror` / `anyhow`: `MIT OR Apache-2.0`

## 5. Consequências

- **Positivas:** binário único `zengravity-desktop.exe` rápido, baixo uso de memória e CPU, canvas 2D fluido para mapas do MU Online, compilação 100% via `cargo` sem depender de Node.js.
- **Cuidados necessários:** em `egui`, não se deve solicitar redesenho contínuo a 60 FPS quando nada mudou se o objetivo for economizar recursos, nem bloquear a thread de UI com I/O de rede ou banco. Toda comunicação com a central/simulador roda em tasks assíncronas do `tokio` comunicando-se por canais limitados (`mpsc` / `watch`), e a UI solicita repaint apenas quando chegam eventos ou em intervalos controlados para atualizar contadores de idade (`stale`).
