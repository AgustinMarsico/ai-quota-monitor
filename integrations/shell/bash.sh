# Source manually. Keeps existing PS1; avoid sourcing twice.
if [[ ${AI_QUOTA_PROMPT_INSTALLED:-0} != 1 ]]; then
    AI_QUOTA_PROMPT_INSTALLED=1
    PS1='$(ai-quota status --compact --color never 2>/dev/null)\n'"$PS1"
fi
