#!/usr/bin/env bash

set -euo pipefail

# ----------------------------------------------------------------------
# Configuration
# ----------------------------------------------------------------------

REMOTE="origin"
DEV_BRANCH="dev"
MAIN_BRANCH="main"
PRODUCTION_BRANCH="production"

# ----------------------------------------------------------------------
# Colors
# ----------------------------------------------------------------------

if [[ -t 1 ]]; then
  BOLD=$'\e[1m'
  DIM=$'\e[2m'
  RESET=$'\e[0m'

  RED=$'\e[31m'
  GREEN=$'\e[32m'
  YELLOW=$'\e[33m'
  BLUE=$'\e[34m'
  CYAN=$'\e[36m'
else
  BOLD=""
  DIM=""
  RESET=""

  RED=""
  GREEN=""
  YELLOW=""
  BLUE=""
  CYAN=""
fi

# ----------------------------------------------------------------------
# Preflight checks
# ----------------------------------------------------------------------

if ! command -v git >/dev/null 2>&1; then
  printf "%sERROR: git is not installed.%s\n" "$RED" "$RESET" >&2
  exit 1
fi

if ! git rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  printf "%sERROR: Not inside a Git repository.%s\n" "$RED" "$RESET" >&2
  exit 1
fi

# ----------------------------------------------------------------------
# Commit message
# ----------------------------------------------------------------------

if [[ $# -gt 0 ]]; then
  COMMIT_MESSAGE="$*"
else
  read -rp "Commit message: " COMMIT_MESSAGE
fi

if [[ -z "$COMMIT_MESSAGE" ]]; then
  printf "%sERROR: Commit message is required.%s\n" "$RED" "$RESET" >&2
  exit 1
fi

# ----------------------------------------------------------------------
# Step tracking
# ----------------------------------------------------------------------

TOTAL_STEPS=11
STEP=0
CURRENT_BRANCH="$(git rev-parse --abbrev-ref HEAD)"

# ----------------------------------------------------------------------
# Logging helpers
# ----------------------------------------------------------------------

start_step() {
  local title="$1"
  local cmd="$2"

  STEP=$((STEP + 1))

  printf "\n%s┌─[%02d/%02d] %s%s\n" \
    "$BLUE" "$STEP" "$TOTAL_STEPS" "$title" "$RESET"

  printf "%s│ $ %s%s\n" \
    "$DIM" "$cmd" "$RESET"
}

ok() {
  printf "%s└─✔ %s%s\n" "$GREEN" "$1" "$RESET"
}

skip() {
  printf "%s└─⏭  %s%s\n" "$YELLOW" "$1" "$RESET"
}

fail() {
  printf "%s└─✖ %s%s\n" "$RED" "$1" "$RESET"
}

abort() {
  local title="$1"
  local status="$2"

  fail "$title"

  printf "%sCommand failed with exit code %s. Check 'git status'.%s\n" \
    "$RED" "$status" "$RESET" >&2

  exit "$status"
}

run() {
  local title="$1"
  shift

  local cmd_display
  cmd_display="$(printf '%q ' "$@")"

  start_step "$title" "${cmd_display% }"

  local status=0
  "$@" || status=$?

  if ((status == 0)); then
    ok "$title"
  else
    abort "$title" "$status"
  fi
}

print_header() {
  local line="══════════════════════════════════════════════════"

  printf "\n%s%s%s%s\n" "$BOLD" "$CYAN" "$line" "$RESET"
  printf "%s%s            Git workflow runner            %s\n" "$BOLD" "$CYAN" "$RESET"
  printf "%s%s%s%s\n" "$BOLD" "$CYAN" "$line" "$RESET"

  printf "Commit message : %s\n" "$COMMIT_MESSAGE"
  printf "Current branch : %s\n" "$CURRENT_BRANCH"
  printf "Remote         : %s\n" "$REMOTE"
}

# ----------------------------------------------------------------------
# Start workflow
# ----------------------------------------------------------------------

print_header

# 1. git add .
run "Stage all changes" \
  git add .

# 2. git commit -m 'commit message'
start_step "Commit staged changes" \
  "$(printf '%q ' git commit -m "$COMMIT_MESSAGE")"

if git diff --cached --quiet; then
  skip "No staged changes to commit"
else
  commit_status=0
  git commit -m "$COMMIT_MESSAGE" || commit_status=$?

  if ((commit_status == 0)); then
    ok "Commit staged changes"
  else
    abort "Commit staged changes" "$commit_status"
  fi
fi

# 3. git push origin dev
run "Push ${DEV_BRANCH} to ${REMOTE}" \
  git push "$REMOTE" "$DEV_BRANCH"

# 4. git merge main
run "Merge ${MAIN_BRANCH} into current branch" \
  git merge --no-edit "$MAIN_BRANCH"

# 5. git checkout main
run "Switch to ${MAIN_BRANCH}" \
  git checkout "$MAIN_BRANCH"

# 6. git merge dev
run "Merge ${DEV_BRANCH} into ${MAIN_BRANCH}" \
  git merge --no-edit "$DEV_BRANCH"

# 7. git push origin main
run "Push ${MAIN_BRANCH} to ${REMOTE}" \
  git push "$REMOTE" "$MAIN_BRANCH"

# 8. git checkout production
run "Switch to ${PRODUCTION_BRANCH}" \
  git checkout "$PRODUCTION_BRANCH"

# 9. git merge main
run "Merge ${MAIN_BRANCH} into ${PRODUCTION_BRANCH}" \
  git merge --no-edit "$MAIN_BRANCH"

# 10. git push origin production
run "Push ${PRODUCTION_BRANCH} to ${REMOTE}" \
  git push "$REMOTE" "$PRODUCTION_BRANCH"

# 11. git checkout dev
run "Switch back to ${DEV_BRANCH}" \
  git checkout "$DEV_BRANCH"

printf "\n%s%s🎉 Workflow completed successfully.%s\n" \
  "$BOLD" "$GREEN" "$RESET"