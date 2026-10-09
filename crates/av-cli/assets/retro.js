/* Retro-theme behaviour.
 *
 * Loaded on every page but only does anything in the retro theme. These are
 * period *ornaments* on real content — the marquee scrolls the actual search
 * status text — not simulated chrome, and everything here respects
 * prefers-reduced-motion.
 *
 * The ornaments are taken down again when the theme leaves retro. The marquee
 * is a second copy of `#results-count` by design, so letting it outlive the
 * theme prints the result count twice under modern, where it has no styling
 * left and reads as a stray line of text rather than as a banner.
 */
'use strict';

(function () {
  const root = document.documentElement;
  let observer = null;
  let heading = null;
  let track = null;

  function install() {
    if (root.dataset.theme !== 'retro') return;
    if (document.getElementById('retro-marquee')) return;

    /* A marquee carrying the live result count. The text is the same string
     * the results heading shows, so it is never decorative fiction. */
    heading = document.getElementById('results-count');
    if (!heading) return;

    const strip = document.createElement('div');
    strip.id = 'retro-marquee';
    strip.className = 'marquee';
    strip.setAttribute('aria-hidden', 'true');

    track = document.createElement('span');
    track.className = 'marquee-track';
    track.textContent = heading.textContent || 'Anta-Vista';
    strip.append(track);

    heading.parentNode.insertBefore(strip, heading);

    observer = new MutationObserver(() => {
      const text = heading.textContent || '';
      if (track.textContent === text) return;
      track.textContent = text;
      /* Restart the animation so a new search is visibly new. */
      track.style.animation = 'none';
      void track.offsetWidth;
      track.style.animation = '';
    });
    observer.observe(heading, {
      childList: true,
      characterData: true,
      subtree: true,
    });
  }

  function uninstall() {
    const strip = document.getElementById('retro-marquee');
    if (strip) strip.remove();
    /* The observer holds the detached track, so it has to go with it — the
     * heading is still live and would keep waking it forever. */
    if (observer) {
      observer.disconnect();
      observer = null;
    }
    heading = null;
    track = null;
    const input = document.getElementById('q');
    if (input) input.classList.remove('typing');
  }

  function sync() {
    if (root.dataset.theme === 'retro') install();
    else uninstall();
  }

  install();

  /* The caret is a class toggle rather than an added element, so it is wired
   * once here instead of inside install(): uninstall/reinstall would otherwise
   * stack listeners. modern.css has no `#q.typing` rule, so it is inert there. */
  const input = document.getElementById('q');
  if (input) {
    input.addEventListener('input', () => input.classList.add('typing'));
    input.addEventListener('blur', () => input.classList.remove('typing'));
  }

  /* The theme can change at runtime, so re-evaluate on toggle — in both
   * directions, not just retro-wards. */
  new MutationObserver(sync).observe(root, {
    attributes: true,
    attributeFilter: ['data-theme'],
  });
})();
