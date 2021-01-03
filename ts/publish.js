var ghpages = require('gh-pages');

ghpages.publish('build', {
  repo: 'ssh://git@github.com/brhoades/vredditor.git',
  message: 'Automatically generated update commit.',
}, console.log);
