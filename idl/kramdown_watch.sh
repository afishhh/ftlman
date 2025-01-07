find output/markdown.md kramdown-template.html |
  entr -s 'echo -n "refreshing... "; kramdown --template kramdown-template.html output/markdown.md >output/rendered.html; echo done'
