# Visão do Produto — ZenGravity

## 1. Propósito

O **ZenGravity** é uma plataforma nativa do Windows para monitoramento em tempo real e coordenação operacional de múltiplos computadores (PCs) e múltiplos personagens de **MU Online**.

Ao contrário do legado (`ZenMonitor`), que dependia de um painel web servido via navegador, o ZenGravity nasce com uma abordagem **Desktop-First e Offline-First**:

1. **Aplicativo Nativo do Windows (`zengravity-desktop.exe`):** abre instantaneamente sem exigir navegador, internet, servidor web externo, Electron ou WebView.
2. **Operação Local e Multi-PC:** uma **Central** recebe telemetria de múltiplos **Agentes** instalados nos PCs da rede local (ou no próprio PC principal), onde cada PC pode executar várias janelas/contas do jogo simultaneamente.
3. **Transparência e Confiabilidade de Dados:** o painel nunca inventa dados. Cada métrica informa sua **origem** (`RAM`, `OCR`, `Tela`, `Simulador`, `Desconhecida`), o **momento da leitura** (idade em segundos) e sua **confiabilidade**.

---

## 2. Princípios Inegociáveis de Domínio e Segurança

### 2.1 Honestidade Epistêmica (Estados Verdadeiros)
O ZenGravity distingue explicitamente os estados operacionais de cada personagem:
- `Farmando`: personagem no spot, helper ativo e Zen subindo dentro do intervalo esperado.
- `Parado`: personagem conectado, porém sem ganho de Zen acima do limiar configurado (`stopped_after_s`).
- `Recuperando`: rotina ativa de retorno ao spot, reposicionamento ou tarefa controlada.
- `Morto`: confirmado por leitura de HP = 0 na memória e/ou confirmação visual na tela, ou evento PK recente não sucedido por renascimento confirmado.
- `Desconectado (DC)`: queda confirmada de conexão TCP aliada à estagnação de Zen e HP.
- `Pausado`: automação ou leitura explicitamente pausada pelo operador (ex.: tecla de parada de emergência F8 ou botão no painel).
- `Leitura Desatualizada (Stale)`: o agente ou a central parou de enviar atualizações recentes dentro da janela de frescor (`stale_after_s`). Os últimos valores conhecidos são marcados visualmente como antigos, sem disparar falsos alertas derivados.
- `Desconhecido`: leitura ainda não inicializada (ex.: trava de RAM pendente de semente OCR ou janela minimizada sem leitura prévia).
- `Online`: conectado e reportando, quando não enquadrado em estado específico de alerta ou farm.

> **Regra de Ouro da Observação:** Ausência de leitura nunca significa ausência de monstros, ausência de jogadores ou spot livre. O cliente do jogo só expõe entidades num raio próximo ao personagem observador; áreas fora do alcance de visão de um personagem online aparecem como **Não Observadas**, nunca como "vazias".

### 2.2 Segurança do Jogo e das Contas
1. **Memória Somente Leitura:** quando o leitor Windows estiver ativo (fases futuras), apenas leituras (`ReadProcessMemory`) são permitidas. É terminantemente proibido escrever na memória do processo, injetar DLLs, interceptar/modificar pacotes de rede ou tentar contornar mecanismos de proteção.
2. **Proteção de Credenciais:** o sistema jamais lê arquivos de configuração do cliente que possam conter credenciais ou senhas em texto puro (como `Settings.ini`).
3. **Controle de Entrada Física (Teclado e Mouse):**
   - Só pode existir **uma sessão de entrada física por PC** por vez, coordenada por um agendador com prioridades claras (sobrevivência > recuperação > conveniência).
   - Toda automação exige habilitação explícita do proprietário, verificação de foco/HWND/PID antes de cada passo, detecção de interferência manual e **botão de parada de emergência** (tanto no painel quanto via atalho físico F8).
   - Em caso de falha de leitura, ambiguidade ou dados velhos, o agente entra em **estado seguro** (não clica nem move o personagem às cegas).
4. **Comandos Fechados:** toda comunicação entre painel, central e agentes usa contratos tipados e enumerados. Não existe comando para executar shell arbitrário ou ler caminhos livres do disco.

---

## 3. Experiência do Usuário no Painel Desktop

- **Tema Escuro Profissional:** alto contraste, tipografia limpa, indicadores cromáticos consistentes para estados e alertas, sem redesenhar ou recapturar dados sem necessidade.
- **Visão Geral Multi-PC e Multicontas:**
  - Cabeçalho com status da conexão da central, modo de operação (ex.: `MODO SIMULAÇÃO` em destaque quando ativo), contadores globais (PCs, personagens, Zen total, taxa total de Zen/h, alertas ativos) e botão global de emergência/pausa.
  - Barra lateral de navegação rápida entre visões (`Visão Geral`, `Mapa 2D & Spots`, `Eventos & Alertas`, `Saúde dos PCs`) e filtros instantâneos por PC, estado, mapa ou busca textual.
  - Cards e tabela de personagens com Zen atual, ganho por hora (`Zen/h`), previsão para o limite de 2 bilhões (`ETA 2B`), HP, vida do Dark Raven (`Pet HP`), mapa/coordenada atual, spot principal (`F1`), status do Helper e origem/idade da leitura.
- **Painel de Detalhes e Gráfico Temporal:** ao selecionar um personagem, exibe série temporal de Zen (sparkline), histórico recente, diagnósticos de trava/leitura, entidades próximas observadas e controles individuais.
- **Mapa 2D Nativo:** canvas interativo com zoom e pan, grid de coordenadas `0..=255`, visualização de zonas seguras e obstáculos conhecidos, posição dos personagens do grupo, spots catalogados (`F1`/`F2`), entidades próximas (NPCs, jogadores, monstros e Golden Monsters) com indicação clara do raio de observação real e da idade da leitura.

---

## 4. Evolução Planejada (Opcionalidade)

- **Mini-painel Flutuante (Overlay):** janela compacta opcional sobre o desktop do Windows, independente da janela do jogo, para acompanhamento rápido.
- **Painel Web e VPS Opcionais:** a arquitetura separa estritamente o domínio e a central da interface gráfica. Um painel web futuro (React + TypeScript + PixiJS) ou publicação remota poderão consumir os mesmos contratos da central sem duplicar regras e sem tornar o navegador obrigatório para o uso local.
