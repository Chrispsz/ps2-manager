#!/usr/bin/env bash
# daily-audit.sh — Auditoria diária do ecossistema PS2 (Mega Prompt 4)
#
# Uso: bash scripts/daily-audit.sh
# Saída: docs/audits/audit-YYYY-MM-DD.md (no repo ps2-manager)
#
# Regras inquebráveis:
#   * nunca force push · nunca commit na main durante a auditoria
#   * vulnerabilidade crítica → branch local fix/security-audit-YYYY-MM-DD (sem push)
#   * token nunca é ecoado em logs
#   * falha de credenciais → registra e segue com o que dá (abortar limpo)
set -uo pipefail
IFS=$'\n\t'

REPO_DIR="$(cd "$(dirname "$0")/.." && pwd)"
export PATH="$HOME/.cargo/bin:$PATH"
MOD_DIR="/home/z/my-project/repos/opl-mod-ptbr"
SECRETS_FILE="/home/z/my-project/.secrets/github_token"
TODAY="$(date +%F)"
OUT="$REPO_DIR/docs/audits"
mkdir -p "$OUT"
REPORT="$OUT/audit-$TODAY.md"

[[ -n "${GITHUB_TOKEN:-}" ]] && TOKEN="$GITHUB_TOKEN"
[[ -z "${TOKEN:-}" && -f "$SECRETS_FILE" ]] && TOKEN="$(cat "$SECRETS_FILE")"

log() { printf '%s\n' "$*" >> "$REPORT"; }

cat > "$REPORT" <<HDR
# Auditoria Diária — $TODAY (Mega Prompt 4 · Governança Contínua)

**Job:** 506890 · **Gerado:** $(date -u '+%F %T UTC') · **Repo:** \`$REPO_DIR\`

HDR

# 0. credenciais
if [[ -n "${TOKEN:-}" ]] && curl -sf -o /dev/null -H "Authorization: Bearer $TOKEN" https://api.github.com/user; then
  log "## 0. Credenciais" "" "✅ Token válido (valor nunca ecoado)"
  AUTH_OK=1
else
  log "## 0. Credenciais" "" "❌ Sem token válido — ciclo segue só offline"
  AUTH_OK=0
fi

# 1. toolchain + cargo
log "" "## 1. Toolchain & Cargo" ""
if command -v cargo >/dev/null 2>&1; then
  ( cd "$REPO_DIR" && cargo update >/dev/null 2>&1 ) && log "✅ cargo update"
  if ( cd "$REPO_DIR" && cargo clippy --all-targets --all-features -- -D warnings >/dev/null 2>&1 ); then
    log "✅ clippy -D warnings"
  else
    log "⚠️ clippy com warnings (ver run manual)"
  fi
  if ( cd "$REPO_DIR" && cargo test --all >/dev/null 2>&1 ); then
    log "✅ cargo test --all"
  else
    log "❌ cargo test falhou"
  fi
  if command -v cargo-audit >/dev/null 2>&1; then
    AUDIT_OUT="$( cd "$REPO_DIR" && cargo audit 2>&1 || true )"
    if echo "$AUDIT_OUT" | rg -qi 'error\[|vulnerability found|Crates with vulnerabilities'; then
      log "❌ cargo audit: VULNERABILIDADES DETECTADAS"
      echo "$AUDIT_OUT" >> "$REPORT"
      BRANCH="fix/security-audit-$TODAY"
      ( cd "$REPO_DIR" && git branch "$BRANCH" >/dev/null 2>&1 ) \
        && log "🛡 branch local criada: \`$BRANCH\` (sem push — regra do protocolo)"
    else
      log "✅ cargo audit: sem vulnerabilidades conhecidas"
    fi
  else
    log "⚠️ cargo-audit ausente (toolchain parcial)"
  fi
else
  log "❌ cargo ausente — 'toolchain ausente' registrado"
fi

# 2. git status dos 2 repos
log "" "## 2. git status (2 repositórios)" ""
for d in "$REPO_DIR" "$MOD_DIR"; do
  if [[ -d "$d/.git" ]]; then
    st="$(git -C "$d" status --porcelain | head -20)"
    br="$(git -C "$d" rev-parse --abbrev-ref HEAD)"
    head_sha="$(git -C "$d" rev-parse --short HEAD)"
    if [[ -z "$st" ]]; then
      log "✅ \`$(basename "$d")\` — branch $br @ $head_sha — working tree limpo"
    else
      log "⚠️ \`$(basename "$d")\` — branch $br @ $head_sha — mudanças não commitadas:" "" '```' "$st" '```'
    fi
  else
    log "❌ \`$d\` ausente (repositório não clonado)"
  fi
done

# 3. upstream OPL (se rede/token)
log "" "## 3. Upstream Open-PS2-Loader" ""
UP_SHA="$(git -C "$MOD_DIR" cat-file -p "$(cat "$MOD_DIR/upstream.sha" 2>/dev/null)" 2>/dev/null | head -c 40 || true)"
UP_PIN="$(cat "$MOD_DIR/upstream.sha" 2>/dev/null || echo '?')"
if [[ "$AUTH_OK" == "1" ]]; then
  LATEST="$(curl -sf -H "Authorization: Bearer $TOKEN" 'https://api.github.com/repos/ps2homebrew/Open-PS2-Loader/branches/master' | jq -r '.commit.sha // "desconhecido"')"
  if [[ -n "$LATEST" && "$LATEST" != "null" && "$LATEST" != "desconhecido" ]]; then
    log "- upstream \`master\`: \`$LATEST\`"
    log "- pin do mod (\`upstream.sha\`): \`$UP_PIN\`"
    [[ "$LATEST" == "$UP_PIN"* ]] && log "✅ mod sincronizado com o upstream" || log "ℹ️ upstream avançou — pipeline semanal cuida do sync"
  else
    log "⚠️ não foi possível consultar o upstream"
  fi
else
  log "⚠️ rede/credenciais indisponíveis — check de upstream pulado"
fi

log "" "---" "" "*Relatório gerado por scripts/daily-audit.sh · token nunca ecoado*"
echo "✅ relatório: $REPORT"
unset TOKEN
