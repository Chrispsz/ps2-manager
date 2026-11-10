#!/usr/bin/env bash
# weekly-release.sh — Release Semanal Integrada do ecossistema PS2 (Mega Prompt 4)
#
# Uso: bash scripts/weekly-release.sh [--dry-run]
#
# Fluxo: bump PATCH no Cargo.toml → CHANGELOG incremental (git log) →
#        commit 'chore(release): vX.Y.Z [skip ci]' → tag anotada →
#        push (commit + tag; o GitHub Actions faz build musl+AppImage e
#        publica a Release com os binários — job 'release' por tag v*).
#
# PRÉ-CONDIÇÕES (qualquer falha → abortar limpo, NUNCA forçar):
#   * credenciais válidas (bootstrap.sh)
#   * working tree limpo nos 2 repos
#   * cargo clippy/test verdes (quando toolchain disponível)
# REGRA: push sem --force; token nunca ecoado nem persistido em .git/config.
set -euo pipefail

REPO_DIR="$(cd "$(dirname "$0")/.." && pwd)"
MOD_DIR="/home/z/my-project/repos/opl-mod-ptbr"
SECRETS_FILE="/home/z/my-project/.secrets/github_token"
DRY_RUN=0
[[ "${1:-}" == "--dry-run" ]] && DRY_RUN=1

export PATH="$HOME/.cargo/bin:$PATH"

# ── credenciais ───────────────────────────────────────────────────────────────
if [[ -n "${GITHUB_TOKEN:-}" ]]; then TOKEN="$GITHUB_TOKEN"
elif [[ -f "$SECRETS_FILE" ]]; then TOKEN="$(cat "$SECRETS_FILE")"
else echo "❌ sem token (GITHUB_TOKEN ou $SECRETS_FILE)"; exit 1; fi

curl -sf -o /dev/null -H "Authorization: Bearer $TOKEN" https://api.github.com/user \
  || { echo "❌ token inválido — abortando limpo"; exit 1; }
echo "✓ credenciais OK"

# ── pré-condições ─────────────────────────────────────────────────────────────
git -C "$REPO_DIR" diff --quiet && git -C "$REPO_DIR" diff --cached --quiet \
  || { echo "❌ ps2-manager com mudanças não commitadas — abortando limpo"; exit 1; }
if [[ -d "$MOD_DIR/.git" ]]; then
  git -C "$MOD_DIR" diff --quiet && git -C "$MOD_DIR" diff --cached --quiet \
    || { echo "❌ opl-mod-ptbr com mudanças não commitadas — abortando limpo"; exit 1; }
fi
echo "✓ working trees limpas"

if command -v cargo >/dev/null 2>&1; then
  ( cd "$REPO_DIR" && cargo clippy --all-targets --all-features -- -D warnings >/dev/null 2>&1 ) \
    || { echo "❌ clippy -D warnings falhou — abortando limpo"; exit 1; }
  ( cd "$REPO_DIR" && cargo test --all >/dev/null 2>&1 ) \
    || { echo "❌ cargo test falhou — abortando limpo"; exit 1; }
  echo "✓ clippy + testes verdes"
else
  echo "⚠ toolchain ausente — CI do GitHub valida o build (aborta se vermelho)"
fi

# ── versão ────────────────────────────────────────────────────────────────────
CUR="$(sed -n 's/^version = "\(.*\)"/\1/p' "$REPO_DIR/Cargo.toml" | head -1)"
MAJ="$(cut -d. -f1 <<<"$CUR")"; MIN="$(cut -d. -f2 <<<"$CUR")"; PAT="$(cut -d. -f3 <<<"$CUR")"
NEXT="${MAJ}.${MIN}.$((PAT + 1))"
LAST_TAG="$(git -C "$REPO_DIR" describe --tags --abbrev=0 2>/dev/null || echo "nenhuma")"

echo ""
echo "═══ PLANO DE RELEASE ═══"
echo "  versão atual : $CUR   (última tag: $LAST_TAG)"
echo "  versão nova  : $NEXT"
echo "  commits novos:"
git -C "$REPO_DIR" log --oneline "${LAST_TAG}..HEAD" 2>/dev/null | sed 's/^/    /' || echo "    (histórico todo)"
if [[ "$DRY_RUN" == "1" ]]; then
  echo ""
  echo "DRY-RUN — nada foi alterado. Rode sem --dry-run para executar."
  exit 0
fi

# ── execução ──────────────────────────────────────────────────────────────────
cd "$REPO_DIR"

sed -i "0,/^version = \"${CUR}\"/s//version = \"${NEXT}\"/" Cargo.toml
# atualiza Cargo.lock (sem mudar deps, só a versão do pacote)
[[ -f Cargo.lock ]] && cargo update -p ps2-manager --quiet 2>/dev/null || true

{
  echo "# CHANGELOG"
  echo ""
  echo "## $NEXT ($(date +%F))"
  git log --oneline "${LAST_TAG}..HEAD" 2>/dev/null | sed 's/^/- /' || echo "- (primeira release)"
  if [[ -f CHANGELOG.md ]]; then
    echo ""
    tail -n +2 CHANGELOG.md
  fi
} > CHANGELOG.md

git add Cargo.toml CHANGELOG.md
# Cargo.lock pode ser ignorado (política bin-only) — só entra se for rastreado
git ls-files --error-unmatch Cargo.lock >/dev/null 2>&1 && git add Cargo.lock || true
# sem [skip ci]: o GitHub suprime workflows de TODO push cujo head commit
# carrega skip — inclusive o push da tag (que é o que dispara o job release)
git commit -q -m "chore(release): ${NEXT}"
git tag -a "v${NEXT}" -m "PS2 Manager v${NEXT}"

PUSH_URL="https://x-access-token:${TOKEN}@github.com/Chrispsz/ps2-manager.git"
git push --quiet "$PUSH_URL" main || { echo "❌ push de main falhou — abortando limpo (tag local fica, sem force)"; exit 1; }
git push --quiet "$PUSH_URL" "v${NEXT}" || { echo "❌ push da tag falhou — abortando limpo (sem force)"; exit 1; }
unset TOKEN

# sincroniza ponteiro da última release do mod (consumido pela auditoria)
if [[ -d "$MOD_DIR/.git" ]]; then
  mkdir -p docs/audits
  MOD_REL="$(git -C "$MOD_DIR" describe --tags --abbrev=0 2>/dev/null || echo 'desconhecida')"
  echo " opl-last-release: ${MOD_REL} ($(date -u '+%F %T UTC'))" > docs/audits/.opl-last-release
  MOD_LATEST=$(curl -sf "https://api.github.com/repos/Chrispsz/OPL-Mod-PTBR/releases" | jq -r '.[0].tag_name // "nenhuma"' 2>/dev/null || echo "desconhecida")
  echo " opl-latest-on-github: ${MOD_LATEST}" >> docs/audits/.opl-last-release
fi

echo ""
echo "🎉 release v${NEXT} enviada — o GitHub Actions vai compilar (musl + AppImage)"
echo "   e publicar a Release com os binários no push da tag (job 'release')."
echo "   Acompanhe: https://github.com/Chrispsz/ps2-manager/actions"
