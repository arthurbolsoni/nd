// Resolve quando o DOM fica `quieto` ms sem mutação, ou ao chegar em `max` ms.
// Devolve {quieto: bool, ms: tempo esperado}.
(quieto, max) => new Promise((resolve) => {
  const inicio = performance.now();
  let ultima = inicio;
  const obs = new MutationObserver(() => { ultima = performance.now(); });
  obs.observe(document, { subtree: true, childList: true, attributes: true, characterData: true });
  const checar = () => {
    const agora = performance.now();
    if (agora - ultima >= quieto || agora - inicio >= max) {
      obs.disconnect();
      resolve({ quieto: agora - ultima >= quieto, ms: Math.round(agora - inicio) });
    } else {
      setTimeout(checar, 50);
    }
  };
  setTimeout(checar, 50);
})
