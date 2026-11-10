#!/usr/bin/env bash
# bootstrap.sh — sanity check de credenciais do ecossistema PS2 (Mega Prompt 4)
#
# Fonte do token (nesta ordem):
#   1. GITHUB_TOKEN no ambiente
#   2. /home/z/my-project/.secrets/github_token  (chmod 600, FORA de qualquer repo)
#
# REGRA: o token NUNCA é ecoado, logado ou persistido em .git/config.
set -euo pipefail

SECRETS_FILE="/home/z/my-project/.secrets/github_token"

if [[ -n "${GITHUB_TOKEN:-}" ]]; then
  TOKEN="$GITHUB_TOKEN"
elif [[ -f "$SECRETS_FILE" ]]; then
  TOKEN="$(cat "$SECRETS_FILE")"
else
  echo "❌ Sem token: defina GITHUB_TOKEN ou crie $SECRETS_FILE (chmod 600)"
  exit 1
fi

code=$(curl -sS -o /dev/null -w '%{http_code}' \
  -H "Authorization: Bearer $TOKEN" https://api.github.com/user)
if [[ "$code" != "200" ]]; then
  echo "❌ Token inválido (HTTP $code) — abortando limpo"
  exit 1
fi

login=$(curl -sS -H "Authorization: Bearer $TOKEN" https://api.github.com/user | jq -r '.login')
echo "✓ bootstrap OK — autenticado como $login (token não ecoado)"

# valida acesso de escrita ao ecossistema
for repo in Chrispsz/ps2-manager Chrispsz/OPL-Mod-PTBR; do
  perms=$(curl -sS -H "Authorization: Bearer $TOKEN" "https://api.github.com/repos/$repo" | jq -r '.permissions.push // false')
  if [[ "$perms" == "true" ]]; then
    echo "✓ $repo: push permitido"
  else
    echo "⚠ $repo: SEM push — releases/tags vão falhar"
  fi
done

unset TOKEN
