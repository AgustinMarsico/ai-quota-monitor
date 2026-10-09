# Source manually. Keeps existing prompt; avoid sourcing twice.
if [[ ${AI_QUOTA_PROMPT_INSTALLED:-0} != 1 ]]; then
    AI_QUOTA_PROMPT_INSTALLED=1
    setopt prompt_subst
    PROMPT='$(ai-quota status --compact --color never 2>/dev/null)'$'\n'"$PROMPT"
fi
