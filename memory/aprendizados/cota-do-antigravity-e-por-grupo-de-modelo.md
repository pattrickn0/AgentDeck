---
titulo: A cota do Antigravity é por grupo de modelo, não do agente
resumo: Argon, os outros Gemini e Claude/GPT esgotam separados; o resumo da barra (o mais gasto) não diz se um modelo responde.
touches: [crates/core/src/usage.rs, crates/core/src/usage/antigravity.rs, crates/core/src/bridge.rs]
---
Em 24/10 o Argon estava com 100% da semana usado, o "Gemini Models" (Flash) com 0,4% e o "Claude and GPT models" com
95%. O resumo `AgentUsage::session/weekly` do Antigravity é o grupo Gemini mais gasto, então diz "esgotado" mesmo com o
Flash livre. Quem decide se um modelo responde é o grupo dele (`usage::group_of`).

**Por quê:** o Claude chamou o `antigravity@arquiteto` no Argon sem cota; o usuário só tinha o Flash 3.8 e os limitados.
Até ali o `ask-agent` não olhava a cota.
**Como aplicar:** antes de delegar, `ask-agent --models <agente>` mostra a linha "cota:" e `--team` o aviso ao lado do
modelo; a ponte recusa (409) a pergunta a um modelo esgotado. Um grupo novo no servidor cai em `group_of` pelo nome:
se aparecer outro (ex.: um Gemini novo com cota própria), acrescente a regra e o teste
`each_model_counts_against_its_own_group`.
