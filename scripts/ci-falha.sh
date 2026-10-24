#!/usr/bin/env bash
# Resume a falha de um passo da CI em anotações (`::error::`). O log de um job só baixa com login no
# GitHub; as anotações saem na API pública (`check-runs/<job>/annotations`), então quem não tem token
# (um agente, a aba GitHub sem credencial) ainda vê por que o job ficou vermelho.
# Uso: scripts/ci-falha.sh <log>
set -u
log="${1:?uso: ci-falha.sh <log>}"
[ -f "$log" ] || { echo "::error::sem log ($log): a falha foi antes dos testes"; exit 0; }

# Sem cores (CARGO_TERM_COLOR=always) e só as linhas que dizem o que quebrou.
relevant=$(sed -E 's/\x1b\[[0-9;]*[A-Za-z]//g' "$log" \
  | sed -n -E '/panicked at/{N;N;N;p;d}; /^(error|warning)(\[|:)|^ *--> |^---- .* ----$|^test .* FAILED$|^ *(left|right):|^error: test failed/p' \
  | head -n 200)
[ -n "$relevant" ] || relevant=$(sed -E 's/\x1b\[[0-9;]*[A-Za-z]//g' "$log" | tail -n 40)

# Uma anotação por bloco de 20 linhas (o GitHub guarda até 10 por passo); `%`, CR e LF escapados.
printf '%s\n' "$relevant" | split -l 20 - "${RUNNER_TEMP:-/tmp}/ci-falha-"
for part in "${RUNNER_TEMP:-/tmp}"/ci-falha-*; do
  msg=$(sed -e 's/%/%25/g' -e 's/\r/%0D/g' "$part" | sed -e ':a' -e 'N' -e '$!ba' -e 's/\n/%0A/g')
  echo "::error title=$(basename "$log")::$msg"
done
