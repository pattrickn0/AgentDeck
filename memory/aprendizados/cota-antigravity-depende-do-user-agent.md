---
titulo: A cota do Antigravity depende do User-Agent
resumo: Só com "antigravity/cli/<versão recente>" o servidor devolve os grupos do Gemini; com outro, só "Claude and GPT models".
touches: [crates/core/src/usage/antigravity.rs, crates/core/src/usage.rs]
---
`retrieveUserQuotaSummary` e `fetchAvailableModels` respondem 200 a qualquer User-Agent, mas o conteúdo muda: com
`antigravity/cli/1.4.5 (...)` vêm "Gemini Models", "Gemini 4 Argon" e "Claude and GPT models"; com `antigravity-cli`,
`antigravity/cli/1.0.0` ou sem versão vem só o de Claude/GPT, e a lista de modelos não traz o Argon. Token, corpo e
projeto não influem.

**Por quê:** até 22/10 o app mandava `antigravity-cli` e mostrava a cota de Claude/GPT como se fosse a do Gemini (0%
com o Argon em 94% na semana).
**Como aplicar:** o pedido imita o CLI com a versão do `agy` instalado (`installed_version`). Se a cota parecer errada,
compare com `agy -p "/quota"`; para ver o que o CLI manda, aponte `CLOUD_CODE_URL` para um servidor local que só
registre o pedido.
