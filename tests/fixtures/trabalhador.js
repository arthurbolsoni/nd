// Worker da página rede.html: busca uma lista e devolve o total.
onmessage = async () => {
  const j = await (await fetch('/api/lista?pagina=7')).json();
  postMessage(j.pagina);
};
