# Pixy Control

Aplicativo nativo de macOS em Rust para a **EMEET PIXY USB 328F:00C0**. Controles diretos por HID/UVC, vídeo por AVFoundation e áudio por CoreAudio. Sem conta, serviço de nuvem ou telemetria. O desfoque de fundo usa segmentação de pessoas e composição local por Apple Vision/Core Image.

## Abrir

Abra **Pixy Control.app**, gerado pelo script de build. Autorize câmera e microfone nas janelas do macOS. A prévia abre durante os ajustes ou quando um aplicativo pede a câmera virtual. Feche o EMEET Studio para liberar o canal HID exclusivo; depois use o **+** no card Câmera para reconectar, se necessário.

Brilho, exposição e anticintilação exigem vídeo ativo nesta Pixy. O app desabilita esses controles sem prévia e confirma as escritas pela leitura do dispositivo.

## Interface

Layout baseado no EMEET Studio: seleção da câmera e abas **Visão / Efeitos / Assistência** à esquerda; prévia com controles sobrepostos à direita; abas **Live / Áudio / V-Cam** abaixo dela. As posições salvas e a posição ao ligar ficam dentro de Visão. A engrenagem no cabeçalho do card Câmera abre serial, firmware e ações do dispositivo. A faixa lateral de navegação foi removida para ampliar a prévia. **Ajustes** abre gravação, arquivos e enquadramento. A área Live reúne os filtros locais; plataformas de transmissão e recursos de beleza foram excluídos.

## Usar no Teams e em outros aplicativos

1. Em **V-Cam**, mantenha **Ativar câmera virtual** ligado.
2. No aplicativo de chamada, selecione **EMEET STUDIO Virtual Camera**. A câmera USB **EMEET PIXY** entrega o vídeo físico e não inclui o zoom do visor nem o blur do Pixy Control.
3. Ajuste enquadramento e, em **Efeitos → Desfocar fundo**, ative o blur e escolha sua intensidade. A imagem processada alimenta a prévia, fotos, gravações e saída virtual.

Mantenha o Pixy Control aberto; ele pode ficar em segundo plano. A saída da extensão instalada é **1920×1080 a 30 fps**, mesmo quando a captura está configurada em4K. Aplicativos de chamada também escolhem sua própria qualidade de transmissão.

Enquadramento, blur e preferências da aplicação são salvos automaticamente. Os controles físicos continuam sendo enviados diretamente à câmera; não há um formato global de captura que obrigue todos os aplicativos a usar4K.

### Privacidade por uso real

Em **V-Cam → Privacidade automática**, o app aguarda3segundos de ociosidade confirmada e seleciona o modo Privacidade. Quando uma câmera virtual é solicitada, retoma o modo Manual ou Rastreamento anterior e o enquadramento configurado. A prévia local enquanto você ajusta a câmera e a gravação também contam como uso. A opção **Ocultar prévia** permite deixar o app na frente sem manter essa prévia como consumidora.

Pausas de até 1,5 segundo no pedido da câmera não encerram a captura; isso evita reinícios durante renegociações de vídeo. A contagem de privacidade começa depois da confirmação de ociosidade.

A detecção inclui a câmera virtual e o uso direto da USB; o próprio produtor virtual não conta como consumidor. Para confirmar ociosidade da USB, o app libera sua captura antes de consultar o estado do sistema. Uma leitura desconhecida nunca é tratada como ociosa.

**Privacidade escolhida manualmente permanece bloqueada**, mesmo que um aplicativo tente usar a câmera. Selecione Manual ou Rastreamento para abrir novamente. O modo anterior lembrado para enquadramento é separado da autorização para sair de uma privacidade que o próprio app ativou.

Pausar a captura ou desativar a saída virtual mantém a sessão de saída conectada com uma imagem neutra; retomar entrega somente frames novos. Isso evita exigir que o aplicativo de chamada selecione a câmera novamente durante a retomada normal.

## Funções

- Manual, rastreamento do firmware e privacidade.
- Joystick, passos direcionais, centralização, inversão dos controles e quatro posições de montagem.
- Zoom da câmera, foco automático/manual, exposição automática/manual em EV, ganho, balanço de branco, brilho, contraste, matiz, saturação, nitidez e anticintilação.
- Oito presets com posição, zoom e miniatura: adicionar, atualizar, chamar e excluir. Posição ao ligar armazenada na câmera.
- Mute, volume, medidor de microfone e os três modos de áudio do firmware.
- Temporizador de privacidade do firmware, gesto de palma e rotação automática.
- Privacidade por uso real, com retorno ao modo anterior e proteção da privacidade manual.
- Desfoque de fundo local por Apple Vision/Core Image, com intensidade ajustável e sujeito preservado.
- Serial, três versões de firmware, remoção da sessão e restauração de fábrica com confirmação.
- Prévia, foto PNG, MP4 com microfone opcional, segmentação de arquivos, rotação, espelhamento, proporção, recorte de quatro cantos com correção de perspectiva, zoom digital de 1× a 4× que preserva a resolução de saída, miniatura de campo completo com arraste e roda do mouse, visor oculto e tela cheia.
- Saída opcional para a câmera virtual EMEET já instalada, por CoreMediaIO, sem executar o EMEET Studio. Requer a extensão local aprovada pelo macOS. A saída usa a configuração BGRA 1920×1080/30 da extensão; outros enquadramentos recebem barras para preservar a proporção.

O modo vertical troca o canvas para 1080×1920 quando a captura é 1920×1080. Girar a imagem preserva o canvas selecionado; o ajuste mantém a proporção com barras, como no Studio.

O rastreamento e os gestos são comandos locais para o firmware da câmera. Plataformas de transmissão, contas, efeitos de beleza e recursos de IA do Studio não fazem parte deste app.

## Arquivos e dependências

Fotos e vídeos vão inicialmente para `~/Movies/Pixy Control`; a pasta e a duração dos segmentos podem ser alteradas em **Ajustes**, abaixo da prévia. Preferências, presets e miniaturas ficam em `~/Library/Application Support/Pixy Control`.

A segmentação oferece 15, 30, 60 e 120 minutos, além de arquivo único. O zoom físico segue as restrições do Studio para 4K/30 e 1080p/60; o zoom digital continua disponível.

Perspectiva, rotação e zoom usam as rotinas SIMD do Apple Accelerate, com buffers de memória limitados. O desfoque segmenta a pessoa antes da rotação, preservando o sujeito também em saídas giradas; usa uma máscara nova por quadro, sem enviar imagens à nuvem. Em4K, processamento e carga do Mac podem reduzir a fluidez em relação a1080p.

No Mac, o vídeo usa o encoder H.264 do hardware via VideoToolbox para liberar CPU. Uma falha ao iniciar esse encoder aparece como erro; o app não troca silenciosamente para uma gravação mais lenta.

Gravação usa o **FFmpeg local** em `/opt/homebrew/bin/ffmpeg`, ou no `PATH`. Prévia e controles físicos não dependem de FFmpeg nem dos binários do Studio. A câmera virtual usa somente a extensão assinada EMEET previamente instalada; este projeto não instala nem distribui essa extensão.

O bundle entregue é local, assinado ad hoc, para Apple Silicon. O projeto utiliza APIs de macOS 14 ou posterior. Foi testado em Apple Silicon com macOS 27. Outras combinações ainda precisam de validação.

## Compilar

```sh
cargo test --locked
cargo clippy --locked -- -D warnings
./build-app.sh
```

O script cria `../Pixy Control.app`. `CARGO_TARGET_DIR` pode apontar para uma pasta externa de compilação.

Diagnósticos explícitos produzem JSON. Feche a janela normal do app antes deles, para não haver dois leitores HID simultâneos:

```sh
open -n "../Pixy Control.app" --args --verify-streams /caminho/absoluto/formatos.json
open -n "../Pixy Control.app" --args --verify-hardware /caminho/absoluto/hardware.json
open -n "../Pixy Control.app" --args --verify-workflows /caminho/absoluto/capturas.json
open -n "../Pixy Control.app" --args --verify-privacy /caminho/absoluto/privacidade.json
open -n "../Pixy Control.app" --args --verify-virtual /caminho/absoluto/camera-virtual.json
open -n "../Pixy Control.app" --args --verify-sharing /caminho/absoluto/retomada-virtual.json
open -n "../Pixy Control.app" --args --verify-fluency /caminho/absoluto/fluidez-virtual.json
open -n "../Pixy Control.app" --args --verify-stress /caminho/absoluto/carga.json
open -n "../Pixy Control.app" --args --verify-startup /caminho/absoluto/inicializacao.json
```

O diagnóstico de hardware altera e restaura controles, incluindo pequenos movimentos do gimbal. Não executa restauração de fábrica nem grava posições de inicialização. O diagnóstico de fluxos salva imagem real e vídeos locais curtos, com e sem áudio. O ciclo de privacidade muda temporariamente o modo e o restaura. O diagnóstico de inicialização salva e limpa a posição apenas quando não existe posição inicial personalizada; posições existentes são preservadas. A restauração de fábrica não faz parte de nenhum diagnóstico.

Os testes com hardware são explícitos e precisam de uma câmera conectada; a CI não verifica hardware ou chamadas.


`--verify-sharing` mantém um produtor e um consumidor durante10ciclos de imagem → privacidade → imagem nova, verificando pixels, timestamps e uso real da extensão. A extensão EMEET mantém uma propriedade customizada desatualizada nos processos duradouros; o app consulta esse metadado em um processo local novo, com cache compartilhado, timeout e saída limitados. Essa consulta não abre câmera nem produtor. `--consume-virtual` e `--consume-physical` são diagnósticos de consumidor independente por20segundos, sem comandos HID e sem iniciar chamadas; podem acompanhar a janela principal. `--blur-image ENTRADA SAIDA RELATORIO` verifica o efeito numa imagem local.

A câmera virtual converte a imagem em um worker separado e publica no relógio de 30 Hz, com uma reserva limitada a dois frames prontos para absorver variações do processamento, sem fila crescente. O diagnóstico `--verify-fluency` usa IDs e CRC nos pixels para medir imagens distintas, repetição, pausas e latência em 1080p/30, 4K/30 e 4K/21. Execute apenas sem consumidores externos: seus padrões aparecem na saída virtual. A taxa de transporte não garante 30 imagens únicas quando o processamento da fonte entrega menos.

A saída reutiliza pixel buffers nativos e a descrição de formato. Quadros repetidos e a imagem neutra reutilizam os mesmos pixels imutáveis; os buffers livres são liberados ao entrar em pausa. A prévia deixa de subir texturas para a GPU quando minimizada ou oculta, e reduz as atualizações da interface nesse estado. A câmera virtual continua recebendo imagens processadas no callback nativo.
