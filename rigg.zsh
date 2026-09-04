# rigg shell integration.  Add to ~/.zshrc:
#   source /path/to/rigg/rigg.zsh
#
# The bindings themselves live in the binary, so they cannot drift from what
# `rigg keys` prints. Equivalent, without this file:
#   source <(rigg keys --shell zsh)
eval "$(rigg keys --shell zsh)"
