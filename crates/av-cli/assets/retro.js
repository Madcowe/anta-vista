/* Retro-theme behaviour.
 *
 * Loaded on every page but only does anything in the retro theme. These are
 * period *ornaments* on real content — the marquee scrolls the actual search
 * status text — not simulated chrome, and everything here respects
 * prefers-reduced-motion.
 */
'use strict';

(function () {
  const root = document.documentElement;

  function install() {
    if (root.dataset.theme !== 'retro') return;
    if (document.getElementById('retro-marquee')) return;

    /* A marquee carrying the live result count. The text is the same string
     * the results heading shows, so it is never decorative fiction. */
    const heading = document.getElementById('results-count');
    if (!heading) return;

    const strip = document.createElement('div');
    strip.id = 'retro-marquee';
    strip.className = 'marquee';
    strip.setAttribute('aria-hidden', 'true');

    const track = document.createElement('span');
    track.className = 'marquee-track';
    track.textContent = heading.textContent || 'Anta-Vista';
    strip.append(track);

    heading.parentNode.insertBefore(strip, heading);

    const sync = () => {
      const text = heading.textContent || '';
      if (track.textContent === text) return;
      track.textContent = text;
      /* Restart the animation so a new search is visibly new. */
      track.style.animation = 'none';
      void track.offsetWidth;
      track.style.animation = '';
    };

    new MutationObserver(sync).observe(heading, {
      childList: true,
      characterData: true,
      subtree: true,
    });

    /* Classic text cursor on the search field. */
    const input = document.getElementById('q');
    if (input) {
      input.addEventListener('input', () => input.classList.add('typing'));
      input.addEventListener('blur', () => input.classList.remove('typing'));
    }
  }

  install();

  /* The theme can change at runtime, so re-evaluate on toggle. */
  new MutationObserver(install).observe(root, {
    attributes: true,
    attributeFilter: ['data-theme'],
  });
})();
