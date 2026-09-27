# Um agente em vários dispositivos

Normalmente o Bastion é uma instalação numa máquina. Com **dispositivos** ele
vira um cérebro só — sua memória, personas e aprovações — compartilhado entre
as máquinas que você tem: uma é o **primário** (a fonte da verdade), as outras
são **nós** que só executam o que o primário manda, confinados, e nunca decidem
nada sozinhos. Se o primário some, você promove um nó e segue.

Este é o lado de produto da spec `multi-device-brain-and-nodes`. O substrato
vive em `bastion-mesh::devices` (Core); esta página é como usar pelo
`bastion-agent`.

## Papéis

- **Primário** — guarda a memória, as personas e as aprovações, serve o web
  app e é o único ponto de política: um nó nunca aprova, nunca escreve memória
  e nunca chama modelo por conta própria.
- **Nó** — disca até o primário (nunca abre porta) e executa os primitivos
  confinados que o dono concedeu: `system.run`, `file.read`, `file.write` e, no
  Windows, os primitivos de UI Automation `ui.snapshot` / `ui.act`.

## Configurar o primário

Na máquina que vai guardar o cérebro:

```
bastion node init
```

Isso cria a chave do dono e um registro de dispositivos só com esta máquina, na
época 1. Depois ligue dispositivos no `bastion.toml` e reinicie o daemon:

```toml
[devices]
enabled = true
# Onde os outros dispositivos alcançam este (publicado para os clientes acharem
# o primário):
address = "https://linux-box.tailnet.ts.net:8443"
# Primário com certificado próprio: o PEM das CAs extras que um nó deve confiar.
# ca_file = "/etc/bastion/primary-ca.pem"
```

O daemon passa a servir `GET /node` (o WebSocket que o nó disca) e a API
`/devices/*`. Alcance pela sua rede privada — o Tailscale é o caminho
documentado; qualquer rota privada com TLS serve. O transporte é TLS por
padrão; `allow_plain_transport = true` libera `ws://` só para uma rota já
cifrada ponta a ponta (uma tailnet) ou testes em loopback.

## Adicionar um nó

No primário, abra **Devices** no web app (ou `POST /devices/pairing-codes` com
o token do daemon) para gerar um código de uso único. Na máquina nova:

```
bastion node pair --primary https://linux-box.tailnet.ts.net:8443 --code BAST-XXXX-XXXX
bastion node run
```

`pair` pede para entrar e espera; você aprova no primário (um dispositivo só é
admitido com a sua assinatura **e** a aprovação de um dispositivo já
registrado). `run` então serve o primário até parar. Um nó começa **sem**
nenhuma concessão — você concede capabilities por dispositivo e, na UI
Automation, por aplicativo.

## Concessões

As capabilities que um nó pode rodar são concedidas por dispositivo (e, no
`ui.act`, por app), pela tela Devices ou `PUT /devices/{id}/grants`. Uma
concessão pode exigir sua aprovação a cada uso; o primário só endurece isso,
nunca afrouxa. Uma concessão revogada ou alterada vale na hora; uma concessão
nova entra na próxima vez que o daemon inicia (a lista de ferramentas faz parte
do prompt em cache do modelo, mantido estável de propósito).

## Réplica, promoção e reconciliação

Um nó marcado como guardando réplica mantém uma cópia cifrada do log de eventos
da memória, atualizada conforme você usa o primário. Se o primário some:

```
bastion node promote
```

transforma a réplica desse nó em primário vivo na próxima época. Quando o
primário antigo volta, ele reentra como nó; o que ele escreveu durante a
partição é mesclado — as crenças são unidas, e uma crença que os dois lados
mudaram vira um **conflito** que você resolve na tela Devices (nenhuma das
versões é descartada até você decidir).

A época cerca o primário antigo: no instante em que ele descobre que existe uma
mais nova, para de aceitar escrita, então dois primários nunca escrevem na
mesma época.

## Segredos

Por padrão **nenhuma** credencial (API key, token) é copiada para um nó. Você
pode deixar um dispositivo guardar segredos escolhidos, dormentes até a
promoção (BMD-18, BMD-29..33):

- No dispositivo que vai guardá-los, defina `BASTION_SECRETS_PASSPHRASE` antes
  do `bastion node pair`. Isso cria a **chave de segredos** do dispositivo,
  embrulhada pela passphrase; só ele a tem, e nunca fica desembrulhada em disco.
- No primário, conceda segredos por dispositivo na tela Devices ou com
  `PUT /devices/{id}/secrets` e `{ "secrets": ["anthropic_api_key", …] }`. O
  primário sela o valor atual de cada segredo concedido para a chave de
  segredos do dispositivo e envia; o nó guarda só o texto cifrado.
- Enquanto for nó, o dispositivo **não consegue** abri-los — não há caminho de
  código que decifre um segredo selado no papel de nó (BMD-30).
- No `bastion node promote`, defina `BASTION_SECRETS_PASSPHRASE` de novo: a
  passphrase desembrulha a chave de segredos e instala os segredos numa pasta
  `promoted-secrets`; aponte `BASTION_SECRETS_DIR` para ela para o daemon usar.
  Esse é o único caminho que os abre (BMD-29), e roda localmente com você
  presente.
- Uma reconexão sela de novo com o valor atual, então uma rotação no primário
  chega ao nó (BMD-31). Revogar um dispositivo apaga os segredos selados e a
  chave de segredos dele, e lista o que girar (BMD-33).

## A casca desktop (Windows)

`desktop/shell` é um app de bandeja que embute o web app do primário numa
WebView2 — não tem interface própria. O **Stop node** da bandeja corta o nó
local na hora. Instale por usuário (sem admin) com `desktop/shell/install.ps1`.
Veja o README dessa pasta.

## Resumo de segurança

- Um nó disca para fora; nunca escuta (BMD-09).
- Um nó só roda capabilities concedidas e recusa ordem de época mais antiga
  (BMD-10, BMD-12).
- Toda chamada remota passa pela autoridade da persona, egress e aprovação do
  primário antes de sair (BMD-11).
- A réplica fica cifrada em disco sob uma chave no cofre do sistema (Windows
  Credential Manager, macOS Keychain; arquivo só-do-dono no Linux).
- Revogar um dispositivo impede a conexão e lista os segredos a girar.
