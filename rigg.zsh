# rigg shell integration.  Source from ~/.zshrc:
#   source /path/to/rigg/rigg.zsh

alias rn='rigg new'
alias ra='rigg attach'
alias rl='rigg logs'
alias rsay='rigg say'
alias rs='rigg stack list'

# ^X a  attach to a stack (picker if you are not standing in one)
rigg-attach-widget() { BUFFER='rigg attach'; zle accept-line }
zle -N rigg-attach-widget
bindkey '^Xa' rigg-attach-widget

# ^X n  start a new task, cursor inside the quotes
rigg-new-widget() {
  BUFFER='rigg new ""'
  CURSOR=$(( ${#BUFFER} - 1 ))
  zle redisplay
}
zle -N rigg-new-widget
bindkey '^Xn' rigg-new-widget

# ^X m  send a follow-up message to a stack
rigg-say-widget() {
  BUFFER='rigg say ""'
  CURSOR=$(( ${#BUFFER} - 1 ))
  zle redisplay
}
zle -N rigg-say-widget
bindkey '^Xm' rigg-say-widget

# ^X l  follow a run's log
rigg-logs-widget() { BUFFER='rigg logs -f'; zle accept-line }
zle -N rigg-logs-widget
bindkey '^Xl' rigg-logs-widget

# ^X s  list stacks without losing what you were typing
rigg-stack-widget() {
  zle push-line
  BUFFER='rigg stack list'
  zle accept-line
}
zle -N rigg-stack-widget
bindkey '^Xs' rigg-stack-widget
