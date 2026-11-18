// Keep every screenshot and link available when JavaScript is disabled.
for (const tour of document.querySelectorAll('.walkthrough')) {
  const steps = [...tour.querySelectorAll('.steps a')];
  const figures = [...tour.querySelectorAll('figure[id]')];

  function show(id) {
    for (const figure of figures) figure.hidden = figure.id !== id;
    for (const link of steps) {
      if (link.hash === '#' + id) link.setAttribute('aria-current', 'step');
      else link.removeAttribute('aria-current');
    }
  }

  function followHash() {
    const match = figures.find(figure => '#' + figure.id === location.hash);
    if (match) show(match.id);
    else if (location.hash === '#' + tour.id) show(figures[0].id);
  }

  show(figures[0].id);
  followHash();
  window.addEventListener('hashchange', followHash);
  for (const link of steps) {
    link.addEventListener('click', event => {
      if (event.ctrlKey || event.metaKey || event.shiftKey || event.altKey || event.button !== 0) return;
      const figure = figures.find(item => '#' + item.id === link.hash);
      if (!figure) return;
      event.preventDefault();
      show(figure.id);
      history.replaceState(null, '', link.hash);
      if (matchMedia('(max-width: 760px)').matches) figure.scrollIntoView({block: 'nearest'});
    });
  }
}
