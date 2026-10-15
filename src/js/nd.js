// Biblioteca do nd na página. Roda num mundo isolado (Page.createIsolatedWorld) de cada quadro:
// a página não vê estas funções nem o registro de refs, e os protótipos dela não interferem.
// Refs ("e12") ficam num registro em memória (ref -> WeakRef do elemento), sem tocar no DOM.
(() => {
  const reg = new Map();
  const refDe = new WeakMap();
  let prox = 1;
  const meuDoc = String(performance.timeOrigin);

  const norm = (s) => (s || '').replace(/\s+/g, ' ').trim();
  const chave = (s) => norm(s).normalize('NFD').replace(/\p{M}/gu, '').toLowerCase();
  const corta = (s, n) => (s.length > n ? s.slice(0, n - 1) + '…' : s);
  const q = (s) => JSON.stringify(s);
  const janela = (el) => el.ownerDocument.defaultView || window;
  const estilo = (el) => janela(el).getComputedStyle(el);

  function refPara(el) {
    const r = refDe.get(el);
    if (r && reg.get(r)?.deref() === el) return r;
    const novo = 'e' + prox++;
    reg.set(novo, new WeakRef(el));
    refDe.set(el, novo);
    return novo;
  }

  function achar(ref) {
    const el = reg.get(ref)?.deref();
    return el && el.isConnected ? el : null;
  }

  function exigir(ref) {
    const el = achar(ref);
    if (!el) throw new Error('ref ' + ref + ' não está mais na página');
    return el;
  }

  // Origem do conteúdo de um iframe (dentro da borda e do padding), no viewport do documento dele.
  function origemConteudo(fe) {
    const r = fe.getBoundingClientRect();
    const cs = estilo(fe);
    return {
      x: r.left + fe.clientLeft + parseFloat(cs.paddingLeft || 0),
      y: r.top + fe.clientTop + parseFloat(cs.paddingTop || 0),
    };
  }

  // Deslocamento do documento de `el` em relação ao viewport deste contexto (iframes do mesmo
  // domínio percorridos por dentro).
  function deslocamento(el) {
    let x = 0, y = 0;
    let w = janela(el);
    while (w && w !== window) {
      const fe = w.frameElement;
      if (!fe) break;
      const o = origemConteudo(fe);
      x += o.x;
      y += o.y;
      w = janela(fe);
    }
    return { x, y };
  }

  // Ancestral seguinte, atravessando shadow DOM; null no body/html (a rolagem da página é à parte).
  function acima(x) {
    const a = x.parentElement || x.getRootNode().host;
    if (!a) return null;
    const d = a.ownerDocument;
    return a === d.body || a === d.documentElement ? null : a;
  }

  // Quanto a rolagem das áreas com rolagem própria em volta de `el` levou o conteúdo para a
  // esquerda e para cima.
  function rolagemDasAreas(el) {
    let x = 0, y = 0;
    for (let a = acima(el); a; a = acima(a)) {
      x += a.scrollLeft;
      y += a.scrollTop;
    }
    return { x, y };
  }

  // Elemento à esquerda da tela ou acima do início da página fica oculto (menu escondido fora da
  // tela), a não ser que tenha ido para lá pela rolagem de uma área própria (lista, painel): aí
  // continua no mapa, como o que a rolagem da página deixa acima da tela.
  function visivel(el, r) {
    if (r.width < 1 || r.height < 1) return false;
    if (!el.checkVisibility({ opacityProperty: true, visibilityProperty: true })) return false;
    const w = janela(el);
    if (r.left >= w.innerWidth) return false;
    if (r.right <= 0 || r.bottom + w.scrollY <= 0) {
      const s = rolagemDasAreas(el);
      if (r.right + s.x <= 0 || r.bottom + w.scrollY + s.y <= 0) return false;
    }
    return true;
  }

  // Tem caixa, não está escondido por CSS e ficou fora da caixa de um ancestral que recorta o
  // conteúdo (overflow): saiu da vista dentro de uma área (carrossel, lista), não da página.
  function recortadoPorArea(el) {
    const b = el.getBoundingClientRect();
    if (b.width < 1 || b.height < 1) return false;
    if (!el.checkVisibility({ opacityProperty: true, visibilityProperty: true })) return false;
    for (let a = acima(el); a; a = acima(a)) {
      const cs = estilo(a);
      if (cs.overflowX === 'visible' && cs.overflowY === 'visible') continue;
      const c = a.getBoundingClientRect();
      if (b.right <= c.left || b.left >= c.right || b.bottom <= c.top || b.top >= c.bottom) return true;
    }
    return false;
  }

  function focoProfundo() {
    let a = document.activeElement;
    for (;;) {
      if (a?.shadowRoot?.activeElement) a = a.shadowRoot.activeElement;
      else if (a && (a.localName === 'iframe' || a.localName === 'frame')) {
        let d = null;
        try { d = a.contentDocument; } catch { /* outro domínio */ }
        if (d?.activeElement && d.activeElement !== d.body) a = d.activeElement;
        else break;
      } else break;
    }
    return a;
  }

  const PAPEIS = new Set([
    'button', 'link', 'checkbox', 'radio', 'switch', 'tab', 'menuitem', 'menuitemcheckbox',
    'menuitemradio', 'option', 'combobox', 'listbox', 'textbox', 'searchbox', 'slider',
    'spinbutton', 'treeitem',
  ]);
  const BOTOES = new Set(['button', 'submit', 'reset', 'image']);
  const DATAS = new Set(['date', 'datetime-local', 'month', 'week', 'time', 'color', 'range']);
  const PULAR = new Set(['script', 'style', 'noscript', 'template', 'head', 'meta', 'link', 'title', 'math']);
  const CAMPOS = new Set(['input', 'select', 'textarea', 'editor', 'combobox', 'textbox', 'searchbox', 'spinbutton']);

  function papel(el) {
    const role = (el.getAttribute('role') || '').trim().split(/\s+/)[0];
    if (role && PAPEIS.has(role)) return role;
    switch (el.localName) {
      case 'a':
        return el.hasAttribute('href') ? 'link' : null;
      case 'button':
      case 'summary':
        return 'button';
      case 'select':
        return 'select';
      case 'textarea':
        return 'textarea';
      case 'input': {
        const t = (el.type || 'text').toLowerCase();
        if (t === 'hidden') return null;
        if (BOTOES.has(t)) return 'button';
        if (t === 'checkbox') return 'checkbox';
        if (t === 'radio') return 'radio';
        return 'input';
      }
    }
    if (el.isContentEditable && !el.parentElement?.isContentEditable) return 'editor';
    return null;
  }

  function clicavel(el) {
    if (el.hasAttribute('onclick')) return true;
    if (el.getAttribute('tabindex') === '0') return true;
    if (estilo(el).cursor !== 'pointer') return false;
    const p = el.parentElement;
    return !(p && estilo(p).cursor === 'pointer');
  }

  // Texto de `el` sem o de controles dentro dele (label com select dentro, por exemplo).
  function textoSem(el) {
    if (!el.querySelector('select, textarea')) return norm(el.innerText);
    const partes = [];
    const tw = el.ownerDocument.createTreeWalker(el, NodeFilter.SHOW_TEXT);
    for (let n = tw.nextNode(); n; n = tw.nextNode()) {
      if (!n.parentElement.closest('select, textarea, option')) partes.push(n.data);
    }
    return norm(partes.join(' '));
  }

  function textoIds(el, attr) {
    const ids = el.getAttribute(attr);
    if (!ids) return '';
    const raiz = el.getRootNode();
    return norm(ids.split(/\s+/).map((id) => {
      const x = raiz.getElementById ? raiz.getElementById(id) : el.ownerDocument.getElementById(id);
      return x ? textoSem(x) || x.textContent : '';
    }).join(' '));
  }

  // Texto curto logo antes do campo, para formulários sem label.
  function textoAnterior(el) {
    for (let x = el, i = 0; x && i < 2; x = x.parentElement, i++) {
      const ant = x.previousElementSibling;
      if (ant && !ant.matches('input, select, textarea, button, a') && !ant.querySelector('input, select, textarea, button')) {
        const t = norm(ant.innerText);
        const rotulo = /^(label|th|td|dt|legend)$/.test(ant.localName) || /[:*?]$/.test(t);
        if (t && t.length <= 40 && rotulo) return t;
        if (t) return '';
      }
    }
    return '';
  }

  function nome(el, p) {
    let n = textoIds(el, 'aria-labelledby') || norm(el.getAttribute('aria-label'));
    if (n) return n;
    if (CAMPOS.has(p) || p === 'checkbox' || p === 'radio' || p === 'switch' || p === 'slider') {
      if (el.labels?.length) n = norm([...el.labels].map(textoSem).join(' '));
      n = n || norm(el.getAttribute('title')) || norm(el.getAttribute('placeholder'))
        || norm(el.getAttribute('aria-placeholder'));
      if (n) return n;
      if (p === 'checkbox' || p === 'radio' || p === 'switch') {
        n = norm(el.innerText) || norm(el.parentElement?.innerText);
        if (n && n.length <= 80) return n;
      }
      return textoAnterior(el) || (el.getAttribute('name') ? 'name=' + el.getAttribute('name') : '');
    }
    if (el.localName === 'input') return norm(el.value) || norm(el.getAttribute('title')) || norm(el.alt);
    n = norm(el.innerText);
    if (n) return n;
    const img = el.querySelector('img[alt]');
    n = norm(img?.alt) || norm(el.getAttribute('title')) || norm(el.querySelector('svg title')?.textContent)
      || norm(el.querySelector('[aria-label]')?.getAttribute('aria-label'));
    return n;
  }

  function marcado(el) {
    if (el.localName === 'input' && (el.type === 'checkbox' || el.type === 'radio')) return el.checked;
    const a = el.getAttribute('aria-checked');
    if (a === 'true' || a === 'mixed') return true;
    if (a === 'false') return false;
    const inp = el.querySelector?.('input[type=checkbox], input[type=radio]');
    return inp ? inp.checked : null;
  }

  function valor(el, p) {
    const tag = el.localName;
    if (tag === 'select') return [...el.selectedOptions].map((o) => norm(o.text)).join(', ');
    if (tag === 'textarea') return el.value;
    if (tag === 'input') {
      const t = (el.type || 'text').toLowerCase();
      if (t === 'checkbox' || t === 'radio' || BOTOES.has(t)) return null;
      if (t === 'file') return [...(el.files || [])].map((f) => f.name).join(', ');
      // Senha: só se tem valor, sem o tamanho. O Edge esconde da página a senha que ele
      // preencheu até um gesto do usuário; `:autofill` mostra que ela está lá.
      if (t === 'password') return el.value || autopreenchido(el) ? '••••' : '';
      return el.value;
    }
    if (p === 'editor') return norm(el.innerText);
    if (p === 'slider' || p === 'spinbutton') return el.getAttribute('aria-valuetext') || el.getAttribute('aria-valuenow');
    return null;
  }

  function resumoOpcoes(el) {
    const ops = [...el.options].map((o) => corta(norm(o.text), 25)).filter(Boolean);
    if (!ops.length) return '';
    const mostra = ops.slice(0, 8).join('|');
    return ' {' + mostra + (ops.length > 8 ? '|… ' + ops.length : '') + '}';
  }

  function flags(el, p, foco) {
    const f = [];
    if (p === 'checkbox' || p === 'radio' || p === 'switch' || p === 'menuitemcheckbox' || p === 'menuitemradio') {
      const m = marcado(el);
      if (m !== null) f.push(m ? 'marcado' : 'desmarcado');
    }
    if (el.disabled || el.getAttribute('aria-disabled') === 'true') f.push('desativado');
    if (el.required || el.getAttribute('aria-required') === 'true') f.push('obrigatório');
    if (el.readOnly && CAMPOS.has(p)) f.push('somente leitura');
    let ex = el.getAttribute('aria-expanded');
    if (el.localName === 'summary' && el.parentElement?.localName === 'details') ex = String(el.parentElement.open);
    if (ex === 'true') f.push('aberto');
    else if (ex === 'false') f.push('fechado');
    if (el.getAttribute('aria-selected') === 'true') f.push('selecionado');
    if (el.getAttribute('aria-pressed') === 'true') f.push('pressionado');
    const cur = el.getAttribute('aria-current');
    if (cur && cur !== 'false') f.push('atual');
    if (el.getAttribute('aria-invalid') === 'true' || el.matches?.(':user-invalid')) f.push('inválido');
    return f;
  }

  function caminhoHref(el) {
    try {
      const u = new URL(el.href, location.href);
      const s = u.origin === location.origin ? u.pathname + u.search : u.host + u.pathname + u.search;
      return corta(s, 70);
    } catch {
      return '';
    }
  }

  function descrever(el, p, op = {}) {
    const nm = corta(nome(el, p), 80);
    let l = p === 'input' ? 'input[' + (el.type || 'text').toLowerCase() + ']' : p;
    if (nm) l += ' ' + q(nm);
    else {
      const dica = el.getAttribute('id') || el.getAttribute('data-testid') || el.getAttribute('name');
      if (dica) l += ' #' + corta(dica, 40);
    }
    if (p === 'link' && (op.href || !nm)) {
      const h = caminhoHref(el);
      if (h) l += ' → ' + h;
    }
    const v = valor(el, p);
    if (v) l += ' = ' + q(corta(v, 60));
    if (el.localName === 'select') l += resumoOpcoes(el);
    const f = flags(el, p, op.foco);
    if (op.oculto) f.push('oculto');
    if (f.length) l += ' [' + f.join(', ') + ']';
    return { p, nm, l };
  }

  function tipoGrupo(el) {
    const role = el.getAttribute('role');
    if (el.localName === 'dialog' || role === 'dialog' || role === 'alertdialog') return 'dialog';
    if (el.localName === 'form' || role === 'form' || role === 'search') return 'form';
    if (el.localName === 'fieldset') return 'fieldset';
    return null;
  }

  function nomeGrupo(el) {
    const n = textoIds(el, 'aria-labelledby') || norm(el.getAttribute('aria-label'))
      || norm(el.querySelector(':scope > legend')?.innerText)
      || norm(el.querySelector('h1, h2, h3, h4, [role=heading]')?.innerText)
      || el.getAttribute('name') || el.getAttribute('id') || '';
    return corta(n, 60);
  }

  function nivelTitulo(el) {
    const m = /^h([1-3])$/.exec(el.localName);
    if (m) return +m[1];
    if (el.getAttribute('role') === 'heading') {
      const n = +(el.getAttribute('aria-level') || 2);
      return n <= 3 ? n : 0;
    }
    return 0;
  }

  const ERRO_CLASSE = /(^|[-_\s])(error|erro|invalid|invalido)([-_\s]|$)/i;

  function ehAviso(el) {
    const role = el.getAttribute('role');
    if (role === 'alert' || role === 'status') return true;
    const live = el.getAttribute('aria-live');
    if (live && live !== 'off') return true;
    const c = typeof el.className === 'string' ? el.className : '';
    return c && ERRO_CLASSE.test(c) && !el.querySelector('input, select, textarea, button');
  }

  function hostDe(src) {
    try {
      const u = new URL(src, location.href);
      return u.protocol.startsWith('http') ? u.host : u.protocol.replace(':', '');
    } catch {
      return src || 'about:blank';
    }
  }

  // Percorre o documento (shadow DOM aberto e iframes do mesmo domínio incluídos) e devolve os
  // itens do mapa. Cada elemento visível com papel interativo, grupo, título e aviso vira um item.
  function percorrer(raiz, op) {
    const itens = [];
    const els = [];
    const vh = window.innerHeight;
    const foco = focoProfundo();
    let modal = null;

    function zona(el) {
      const r = el.getBoundingClientRect();
      const y = r.top + deslocamento(el).y;
      if (y + r.height < -vh) return -1;
      if (y > 2 * vh) return 1;
      return 0;
    }

    function add(el, item) {
      itens.push(item);
      els.push(el);
    }

    function arquivosOcultos(el, n, m) {
      const fs = el.matches('input[type=file]') ? [el] : el.querySelectorAll('input[type=file]');
      for (const f of fs) {
        const d = descrever(f, 'input', { oculto: true, foco });
        add(f, { r: refPara(f), t: 'el', n, l: d.l, z: zona(el), m, p: d.p, nm: d.nm });
      }
    }

    function andar(el, n, m, dentroInterativo, dentroAviso) {
      if (PULAR.has(el.localName)) return;
      const p = papel(el);
      if (!el.checkVisibility()) {
        if (estilo(el).display === 'contents') {
          filhos(el, n, m, dentroInterativo, dentroAviso);
        } else if (p === 'checkbox' || p === 'radio') {
          rotuloVisivel(el, n, m);
        } else {
          arquivosOcultos(el, n, m);
        }
        return;
      }
      const r = el.getBoundingClientRect();
      const tag = el.localName;

      if (tag === 'iframe' || tag === 'frame') {
        if (!visivel(el, r) || r.width < 20 || r.height < 20) return;
        let doc = null;
        try { doc = el.contentDocument; } catch { /* outro domínio */ }
        const host = hostDe(el.getAttribute('src') || el.src);
        if (doc?.documentElement) {
          const i = itens.length;
          add(el, { r: refPara(el), t: 'grupo', n, l: 'iframe ' + q(host), z: zona(el), m, p: 'iframe', nm: host });
          andar(doc.documentElement, n + 1, m, false, false);
          if (itens.length === i + 1) { itens.pop(); els.pop(); }
        } else {
          add(el, { r: refPara(el), t: 'quadro', n, l: 'iframe ' + q(host), z: zona(el), m, p: 'iframe', nm: host });
        }
        return;
      }

      const g = tipoGrupo(el);
      let filhoN = n;
      let mFilho = m;
      let iGrupo = -1;
      if (g && visivel(el, r)) {
        const nm = nomeGrupo(el);
        const ehModal = g === 'dialog' && (el.matches(':modal') || el.getAttribute('aria-modal') === 'true');
        if (ehModal) mFilho = true;
        iGrupo = itens.length;
        const ref = refPara(el);
        add(el, { r: ref, t: 'grupo', n, l: g + (nm ? ' ' + q(nm) : ''), z: zona(el), m: mFilho, p: g, nm });
        if (ehModal) modal = ref;
        filhoN = n + 1;
      }

      const nt = nivelTitulo(el);
      let aviso = dentroAviso;
      if (nt && visivel(el, r)) {
        const t = norm(el.innerText);
        if (t) add(el, { r: null, t: 'titulo', n: filhoN, l: 'h' + nt + ' ' + corta(t, 100), z: zona(el), m: mFilho });
      } else if (!dentroAviso && ehAviso(el)) {
        const t = norm(el.innerText);
        if (t && t.length <= 300 && visivel(el, r)) {
          add(el, { r: null, t: 'aviso', n: filhoN, l: 'aviso ' + q(corta(t, 150)), z: zona(el), m: mFilho });
        }
        aviso = true;
      }

      let interativo = dentroInterativo;
      if (p) {
        let oculto = false;
        let incluir = visivel(el, r);
        if (!incluir && tag === 'input' && el.type === 'file') incluir = oculto = true;
        if (!incluir && (p === 'checkbox' || p === 'radio')) incluir = temRotuloVisivel(el);
        if (incluir) {
          const d = descrever(el, p, { foco, href: op.href, oculto });
          add(el, { r: refPara(el), t: 'el', n: filhoN, l: d.l, z: zona(el), m: mFilho, p: d.p, nm: d.nm });
        }
        interativo = true;
        if (tag === 'select' || tag === 'input' || tag === 'textarea') return;
      } else if (!dentroInterativo && !g && tag !== 'svg' && visivel(el, r) && clicavel(el)) {
        const nm = corta(nome(el, 'clicável'), 80);
        if (nm) {
          add(el, { r: refPara(el), t: 'el', n: filhoN, l: 'clicável ' + q(nm), z: zona(el), m: mFilho, p: 'clicável', nm });
          interativo = true;
        }
      }

      if (tag !== 'svg') filhos(el, filhoN, mFilho, interativo, aviso);
      if (iGrupo >= 0 && itens.length === iGrupo + 1) {
        itens.pop();
        els.pop();
        if (modal === refDe.get(el)) modal = null;
      }
    }

    function filhos(el, n, m, interativo, aviso) {
      if (el.shadowRoot) for (const c of el.shadowRoot.children) andar(c, n, m, interativo, aviso);
      for (const c of el.children) andar(c, n, m, interativo, aviso);
    }

    function temRotuloVisivel(el) {
      return [...(el.labels || [])].some((lb) => visivel(lb, lb.getBoundingClientRect()));
    }

    function rotuloVisivel(el, n, m) {
      if (!temRotuloVisivel(el)) return;
      const d = descrever(el, papel(el), { foco });
      add(el, { r: refPara(el), t: 'el', n, l: d.l, z: zona(el), m, p: d.p, nm: d.nm });
    }

    andar(raiz, 0, false, false, false);

    // k: posição entre os itens de mesmo papel e nome (localizador de reserva).
    const cont = new Map();
    for (const it of itens) {
      if (it.t !== 'el') continue;
      const c = it.p + '|' + it.nm;
      it.k = cont.get(c) || 0;
      cont.set(c, it.k + 1);
    }
    return { itens, els, modal };
  }

  function ajustarProx(a) {
    if (!(a.principal && a.doc !== meuDoc)) prox = Math.max(prox, a.prox || 1);
  }

  // Linhas distintas do texto visível do documento (para o diff mostrar texto novo).
  function linhasTexto() {
    const vistas = new Set();
    for (const l of textoProfundo().split('\n')) {
      const t = corta(norm(l), 200);
      if (t) vistas.add(t);
      if (vistas.size >= 3000) break;
    }
    return [...vistas];
  }

  // Refs dadas antes que ficaram fora do mapa mas seguem no documento, recortadas por uma área
  // (carrossel por transform, por exemplo). Limpa do registro as refs de elementos já coletados.
  function foraDaVista(itens) {
    const listados = new Set(itens.map((it) => it.r).filter(Boolean));
    const fora = [];
    for (const [r, wr] of reg) {
      const el = wr.deref();
      if (!el) reg.delete(r);
      else if (!listados.has(r) && el.isConnected && recortadoPorArea(el)) fora.push(r);
    }
    return fora;
  }

  function mapa(a) {
    ajustarProx(a);
    const raiz = a.dentro ? exigir(a.dentro) : document.documentElement;
    const { itens, modal } = percorrer(raiz, a);
    const txt = a.texto ? linhasTexto() : undefined;
    const fora = a.texto && !a.dentro ? foraDaVista(itens) : undefined;
    return { itens, modal, prox, doc: meuDoc, url: location.href, titulo: typeof document.title === 'string' ? document.title : (document.querySelector('title')?.textContent || ''), txt, fora };
  }

  // Acha de novo um elemento que o re-render trocou: mesmo papel e nome (e posição entre iguais).
  function localizar(a) {
    ajustarProx(a);
    const { itens, els } = percorrer(document.documentElement, {});
    const iguais = [];
    itens.forEach((it, i) => { if (it.t === 'el' && it.p === a.p && it.nm === a.nm) iguais.push(i); });
    const i = iguais.length === 1 ? iguais[0] : iguais.find((j) => itens[j].k === a.k);
    if (i === undefined) return { achou: false, prox, doc: meuDoc };
    const el = els[i];
    reg.set(a.ref, new WeakRef(el));
    refDe.set(el, a.ref);
    return { achou: true, l: itens[i].l, prox, doc: meuDoc };
  }

  // Fora (mesmo em parte) da caixa de uma área com rolagem própria em volta dele.
  function foraDaArea(el, b) {
    for (let a = acima(el); a; a = acima(a)) {
      if (a.scrollHeight <= a.clientHeight + 1 && a.scrollWidth <= a.clientWidth + 1) continue;
      const cs = estilo(a);
      if (!/(auto|scroll|overlay)/.test(cs.overflowY + ' ' + cs.overflowX)) continue;
      const c = a.getBoundingClientRect();
      if (b.top < c.top - 1 || b.left < c.left - 1 || b.bottom > c.bottom + 1 || b.right > c.right + 1) return true;
    }
    return false;
  }

  function rolarSePreciso(el) {
    const b = el.getBoundingClientRect();
    const w = janela(el);
    if (b.top < 0 || b.left < 0 || b.bottom > w.innerHeight || b.right > w.innerWidth || foraDaArea(el, b)) {
      el.scrollIntoView({ block: 'center', inline: 'center', behavior: 'instant' });
    }
  }

  // `hit` é `el`, está dentro dele (atravessando shadow DOM) ou é um label dele.
  function atinge(el, hit) {
    for (let x = hit; x; x = x.parentNode || x.host) {
      if (x === el) return true;
    }
    if (el.labels) for (const lb of el.labels) if (lb === hit || lb.contains(hit)) return true;
    return false;
  }

  function descrCurta(el) {
    const p = papel(el);
    if (p) return descrever(el, p).l;
    const t = corta(norm(el.innerText), 50);
    return el.localName + (el.getAttribute('id') ? '#' + el.getAttribute('id') : '') + (t ? ' ' + q(t) : '');
  }

  // Ponto para o clique, no viewport deste contexto. Testa o centro e outros pontos da caixa com
  // elementFromPoint; se todos caem em outro elemento, devolve quem cobre.
  function ponto(a) {
    ajustarProx(a);
    const el = exigir(a.ref);
    rolarSePreciso(el);
    const b = el.getBoundingClientRect();
    if (b.width < 1 || b.height < 1) return { semCaixa: true, prox };
    const w = janela(el);
    const off = deslocamento(el);
    const raiz = el.getRootNode();
    const pts = [[0.5, 0.5], [0.3, 0.3], [0.7, 0.3], [0.3, 0.7], [0.7, 0.7], [0.5, 0.15], [0.5, 0.85]];
    let quem = null;
    for (const [fx, fy] of pts) {
      const x = b.left + b.width * fx;
      const y = b.top + b.height * fy;
      if (x < 0 || y < 0 || x >= w.innerWidth || y >= w.innerHeight) continue;
      const hit = raiz.elementFromPoint(x, y);
      if (hit && atinge(el, hit)) return { x: x + off.x, y: y + off.y, prox };
      if (hit && !quem) quem = hit;
    }
    if (!quem) return { foraDaTela: true, prox };
    return { coberto: { r: refPara(quem), l: descrCurta(quem) }, prox };
  }

  // Origem do conteúdo do iframe `ref` neste contexto, rolando para deixar visível o ponto
  // (px, py) de dentro dele.
  function caixaQuadro(a) {
    const fe = exigir(a.ref);
    const w = janela(fe);
    let o = origemConteudo(fe);
    const alvoY = o.y + (a.py || 0);
    const alvoX = o.x + (a.px || 0);
    if (alvoY < 0 || alvoY >= w.innerHeight || alvoX < 0 || alvoX >= w.innerWidth) {
      fe.scrollIntoView({ block: 'start', inline: 'nearest', behavior: 'instant' });
      o = origemConteudo(fe);
      const y = o.y + (a.py || 0);
      if (y >= w.innerHeight) w.scrollBy(0, y - w.innerHeight / 2);
      o = origemConteudo(fe);
    }
    const off = deslocamento(fe);
    return { x: o.x + off.x, y: o.y + off.y };
  }

  // Campo editável de verdade quando a ref é um label, um wrapper ou um combobox em volta do input.
  function campoDe(el) {
    if (el.localName === 'label' && el.control) return el.control;
    if (['input', 'textarea', 'select'].includes(el.localName) || el.isContentEditable) return el;
    return el.querySelector('input:not([type=hidden]):not([type=checkbox]):not([type=radio]), textarea, select, [contenteditable=""], [contenteditable=true]') || el;
  }

  function tipoCampo(el) {
    const tag = el.localName;
    if (tag === 'select') return 'select';
    if (tag === 'textarea') return 'texto';
    if (tag === 'input') {
      const t = (el.type || 'text').toLowerCase();
      if (t === 'file') return 'arquivo';
      if (DATAS.has(t)) return 'direto';
      if (t === 'checkbox' || t === 'radio' || BOTOES.has(t)) return 'outro';
      return 'texto';
    }
    if (el.isContentEditable) return 'editor';
    return 'outro';
  }

  function tipo(a) {
    const el = exigir(a.ref);
    return { tipo: tipoCampo(campoDe(el)), papel: papel(el), marcado: marcado(el) };
  }

  // Foca o campo e seleciona o conteúdo, para o texto inserido substituir o que havia.
  function preparar(a) {
    const el = campoDe(exigir(a.ref));
    const t = tipoCampo(el);
    rolarSePreciso(el);
    el.focus({ preventScroll: true });
    if (t === 'texto') {
      try { el.select(); } catch { /* tipo sem seleção */ }
    } else if (t === 'editor') {
      el.ownerDocument.getSelection().selectAllChildren(el);
    }
    return { tipo: t, foco: focoProfundo() === el };
  }

  // Define o valor pelo setter nativo e dispara input/change (frameworks com input controlado
  // aceitam, porque o rastreador deles fica com o valor antigo).
  function definir(a) {
    const el = campoDe(exigir(a.ref));
    const w = janela(el);
    if (el.isContentEditable) {
      el.textContent = a.valor;
      el.dispatchEvent(new w.InputEvent('input', { bubbles: true, composed: true, inputType: 'insertText', data: a.valor }));
    } else {
      const proto = el.localName === 'textarea' ? w.HTMLTextAreaElement.prototype
        : el.localName === 'select' ? w.HTMLSelectElement.prototype : w.HTMLInputElement.prototype;
      Object.getOwnPropertyDescriptor(proto, 'value').set.call(el, a.valor);
      el.dispatchEvent(new w.Event('input', { bubbles: true, composed: true }));
      el.dispatchEvent(new w.Event('change', { bubbles: true }));
    }
    return estado(a);
  }

  function estado(a) {
    const el = exigir(a.ref);
    const c = campoDe(el);
    const p = papel(el) || 'clicável';
    let v = valor(c, papel(c) || p);
    // Valor real só para conferir o fill; o daemon não o imprime (`senha`).
    const senha = c.localName === 'input' && c.type === 'password';
    if (senha) v = c.value;
    return { l: descrever(el, p, { foco: focoProfundo() }).l, valor: v, marcado: marcado(el), senha };
  }

  function autopreenchido(el) {
    for (const sel of [':autofill', ':-webkit-autofill']) {
      try {
        if (el.matches(sel)) return true;
      } catch { /* seletor desconhecido */ }
    }
    return false;
  }

  const ENVIAR = /(^|[^\p{L}])(entrar|login|log in|sign in|signin|acessar|continuar|continue|next|avançar|próximo|prosseguir|ok)([^\p{L}]|$)/iu;
  const enviaTexto = (b) => {
    const t = norm(b.innerText || b.value || b.getAttribute('aria-label'));
    return t.length <= 60 && ENVIAR.test(t);
  };

  // Formulário de login do quadro: o primeiro campo de senha visível, o campo de usuário antes
  // dele e o botão de envio. Diz se cada campo tem valor (ou foi preenchido pelo navegador) sem
  // devolver o valor.
  function formLogin(a) {
    ajustarProx(a);
    const vis = (el) => !el.disabled && visivel(el, el.getBoundingClientRect());
    const senha = todos('input[type=password]').find(vis);
    if (!senha) return { achou: false, prox };
    const caixa = senha.form || senha.closest('form, dialog, [role=dialog]') || senha.ownerDocument;
    const textos = [...caixa.querySelectorAll('input')].filter((el) => {
      const t = (el.getAttribute('type') || 'text').toLowerCase();
      return ['text', 'email', 'tel', ''].includes(t) && vis(el);
    });
    const antes = textos.filter((el) => el.compareDocumentPosition(senha) & Node.DOCUMENT_POSITION_FOLLOWING);
    const usuario = antes[antes.length - 1] || null;
    const botoes = [...caixa.querySelectorAll('button, input[type=submit], input[type=image], [role=button]')].filter(vis);
    const botao = botoes.find((b) => b.type === 'submit' && b.form === senha.form && senha.form)
      || botoes.find(enviaTexto)
      || null;
    const tem = (el) => !!el && (el.value.length > 0 || autopreenchido(el));
    const ac = (el) => (el && el.getAttribute('autocomplete')) || '';
    return {
      achou: true,
      prox,
      senha: refPara(senha),
      usuario: usuario && refPara(usuario),
      botao: botao && refPara(botao),
      descrBotao: botao ? descrCurta(botao) : null,
      senhaOk: tem(senha),
      usuarioOk: !usuario || tem(usuario),
      senhaAuto: autopreenchido(senha),
      usuarioAuto: !!usuario && autopreenchido(usuario),
      bloqueado: /new-password|off/i.test(ac(senha)) || /new-password|off/i.test(ac(usuario)),
    };
  }

  // Primeira etapa de login em duas telas (usuário, depois senha): um formulário com um campo de
  // texto/e-mail/telefone visível e um botão de continuar.
  function etapaUsuario(a) {
    ajustarProx(a);
    const vis = (el) => !el.disabled && visivel(el, el.getBoundingClientRect());
    for (const f of todos('form')) {
      const campos = [...f.querySelectorAll('input')].filter((el) => {
        const t = (el.getAttribute('type') || 'text').toLowerCase();
        return ['text', 'email', 'tel', ''].includes(t) && vis(el);
      });
      if (campos.length !== 1) continue;
      const botoes = [...f.querySelectorAll('button, input[type=submit], [role=button]')].filter(vis);
      const botao = botoes.find(enviaTexto) || botoes.find((b) => b.type === 'submit') || null;
      return { achou: true, prox, usuario: refPara(campos[0]), botao: botao && refPara(botao) };
    }
    return { achou: false, prox };
  }

  // Site que marca o login com autocomplete="new-password"/"off" (gov.br) faz o navegador não
  // oferecer a senha salva. A dica vira current-password/username; com `clonar`, o campo é
  // recriado para o navegador analisar o formulário de novo.
  function liberarSenhaSalva(a) {
    ajustarProx(a);
    const senha = exigir(a.senha);
    const usuario = a.usuario ? achar(a.usuario) : null;
    let mudou = false;
    for (const [el, dica] of [[senha, 'current-password'], [usuario, 'username']]) {
      if (el && /new-password|off/i.test(el.getAttribute('autocomplete') || '')) {
        el.setAttribute('autocomplete', dica);
        mudou = true;
      }
    }
    let novo = senha;
    if (a.clonar) {
      novo = senha.cloneNode(true);
      senha.replaceWith(novo);
    }
    return { mudou, senha: refPara(novo), prox };
  }

  function melhor(texto, candidatos) {
    const k = chave(texto);
    const pont = (t) => {
      const c = chave(t);
      if (!c) return 0;
      if (c === k) return 3;
      if (c.startsWith(k)) return 2;
      if (c.includes(k)) return 1;
      return 0;
    };
    let melhorI = -1, melhorP = 0;
    candidatos.forEach((t, i) => {
      const p = pont(t);
      if (p > melhorP) { melhorP = p; melhorI = i; }
    });
    return melhorI;
  }

  // Escolhe uma opção de <select> nativo por texto ou value.
  function escolher(a) {
    const el = campoDe(exigir(a.ref));
    const ops = [...el.options];
    let i = ops.findIndex((o) => o.value === a.opcao);
    if (i < 0) i = melhor(a.opcao, ops.map((o) => o.text));
    if (i < 0) return { erro: true, opcoes: ops.slice(0, 40).map((o) => norm(o.text)) };
    const w = janela(el);
    Object.getOwnPropertyDescriptor(w.HTMLSelectElement.prototype, 'selectedIndex').set.call(el, i);
    el.dispatchEvent(new w.Event('input', { bubbles: true, composed: true }));
    el.dispatchEvent(new w.Event('change', { bubbles: true }));
    return { texto: norm(ops[i].text) };
  }

  function todos(sel, raiz = document) {
    const out = [...raiz.querySelectorAll(sel)];
    for (const h of raiz.querySelectorAll('*')) if (h.shadowRoot) out.push(...todos(sel, h.shadowRoot));
    return out;
  }

  const SEL_OPCOES = [
    '[role=option]', '[role=menuitem]', '[role=menuitemradio]', '[role=menuitemcheckbox]',
    '[role=treeitem]', '[role=listbox] li', '[role=menu] li', '.select2-results__option',
    '[class*="option"]:not(:has([class*="option"]))',
  ].join(', ');

  // Opções visíveis de lista customizada (combobox, menu, autocomplete); a melhor para `texto`.
  function opcoes(a) {
    ajustarProx(a);
    const vistos = new Set();
    const lista = [];
    for (const el of todos(SEL_OPCOES)) {
      if (vistos.has(el) || el.localName === 'input') continue;
      vistos.add(el);
      const r = el.getBoundingClientRect();
      if (!visivel(el, r)) continue;
      const t = norm(el.innerText) || norm(el.getAttribute('aria-label'));
      if (t) lista.push({ el, t });
    }
    const i = melhor(a.texto, lista.map((x) => x.t));
    // Campo de busca da própria lista (focado ao abrir): filtra lista virtualizada.
    const f = focoProfundo();
    const busca = !!f && f !== (a.ref ? achar(a.ref) : null)
      && ((f.localName === 'input' && /^(text|search)$/.test(f.type)) || f.getAttribute('role') === 'searchbox');
    return {
      escolhida: i >= 0 ? { r: refPara(lista[i].el), t: lista[i].t } : null,
      textos: lista.slice(0, 20).map((x) => corta(x.t, 60)),
      busca,
      prox,
    };
  }

  function clicarJs(a) {
    exigir(a.ref).click();
    return true;
  }

  function focar(a) {
    const el = campoDe(exigir(a.ref));
    rolarSePreciso(el);
    el.focus({ preventScroll: true });
    return focoProfundo() === el;
  }

  function textoProfundo(raiz = document.body) {
    if (!raiz) return '';
    let t = raiz.innerText || '';
    for (const h of raiz.querySelectorAll('*')) {
      if (h.shadowRoot) for (const c of h.shadowRoot.children) t += '\n' + textoProfundo(c);
    }
    return t;
  }

  function temTexto(a) {
    return chave(textoProfundo()).includes(chave(a.texto));
  }

  function temSel(a) {
    return todos(a.sel).some((el) => visivel(el, el.getBoundingClientRect()));
  }

  // Elemento visível mais interno cujo texto bate com `texto` (exato vence parcial).
  function porTexto(a) {
    ajustarProx(a);
    const k = chave(a.texto);
    let parcial = null;
    const tw = document.createTreeWalker(document.body, NodeFilter.SHOW_TEXT);
    for (let n = tw.nextNode(); n; n = tw.nextNode()) {
      if (!chave(n.data).includes(k)) continue;
      const el = n.parentElement;
      if (!el || PULAR.has(el.localName) || !visivel(el, el.getBoundingClientRect())) continue;
      if (chave(el.innerText) === k) return { r: refPara(el), l: descrCurta(el), prox };
      if (!parcial) parcial = el;
    }
    return parcial ? { r: refPara(parcial), l: descrCurta(parcial), prox } : { r: null, prox };
  }

  // Refaz uma chamada com fetch daqui (mesma origem e cookies da página). Com `base64`, devolve os
  // bytes da resposta em base64 (arquivo binário).
  async function api(a) {
    try {
      const r = await fetch(a.url, {
        method: a.metodo,
        headers: a.cabecalhos,
        body: a.corpo ?? undefined,
        credentials: 'include',
      });
      const tipo = r.headers.get('content-type') || '';
      if (a.base64) {
        const buf = new Uint8Array(await r.arrayBuffer());
        let bin = '';
        for (let i = 0; i < buf.length; i += 0x8000) bin += String.fromCharCode.apply(null, buf.subarray(i, i + 0x8000));
        return { status: r.status, tipo, b64: btoa(bin), tamanho: buf.length, url: r.url, disp: r.headers.get('content-disposition') };
      }
      const t = await r.text();
      return { status: r.status, tipo, texto: t.length > a.max ? t.slice(0, a.max) : t, tamanho: t.length, url: r.url };
    } catch (e) {
      return { erro: String(e) };
    }
  }

  // `el` ou o ancestral mais próximo (atravessando shadow DOM) com rolagem vertical própria; null
  // quando só a página rola.
  function rolavel(el) {
    for (let x = el; x && x.nodeType === 1; x = x.parentElement || x.getRootNode().host) {
      const d = x.ownerDocument;
      if (x === d.documentElement || x === d.body) return null;
      if (/(auto|scroll|overlay)/.test(estilo(x).overflowY) && x.scrollHeight > x.clientHeight + 1) return x;
    }
    return null;
  }

  // Ponto do viewport onde o gesto rola a página direto: fora de listas com rolagem própria e de
  // iframes.
  function pontoPagina() {
    const pts = [[0.5, 0.5], [0.5, 0.25], [0.5, 0.75], [0.25, 0.5], [0.75, 0.5], [0.1, 0.5], [0.9, 0.5], [0.5, 0.1]];
    for (const [fx, fy] of pts) {
      const x = innerWidth * fx;
      const y = innerHeight * fy;
      const hit = document.elementFromPoint(x, y);
      if (!hit || (hit.localName !== 'iframe' && !rolavel(hit))) return { x, y };
    }
    return { x: innerWidth / 2, y: innerHeight / 2 };
  }

  // Área que `nd scroll` rola: a página, ou com `ref` o elemento (ou o ancestral) com rolagem
  // própria. Devolve o ponto do gesto (centro da parte visível), a posição e a altura total.
  function rolagem(a) {
    ajustarProx(a);
    if (!a.ref) {
      const e = document.scrollingElement || document.documentElement;
      return { ...pontoPagina(), top: e.scrollTop, cliente: innerHeight, total: e.scrollHeight, prox };
    }
    const el = rolavel(exigir(a.ref));
    if (!el) return { semRolagem: true, prox };
    const w = janela(el);
    let b = el.getBoundingClientRect();
    if (b.bottom <= 0 || b.top >= w.innerHeight) {
      el.scrollIntoView({ block: 'center', behavior: 'instant' });
      b = el.getBoundingClientRect();
    }
    const top = Math.max(b.top, 0);
    const bottom = Math.min(b.bottom, w.innerHeight);
    const left = Math.max(b.left, 0);
    const right = Math.min(b.right, w.innerWidth);
    const off = deslocamento(el);
    return {
      r: refPara(el), l: descrCurta(el),
      x: (left + right) / 2 + off.x, y: (top + bottom) / 2 + off.y,
      top: el.scrollTop, cliente: el.clientHeight, total: el.scrollHeight, prox,
    };
  }

  // Espera o DOM ficar `ms` sem mudança de estrutura ou texto, até `max` ms.
  function quieto(a) {
    return new Promise((resolve) => {
      const inicio = performance.now();
      let ultima = inicio;
      const obs = new MutationObserver(() => { ultima = performance.now(); });
      // Só estrutura e texto: atributos mudam o tempo todo em carrossel e animação.
      obs.observe(document, { subtree: true, childList: true, characterData: true });
      const checar = () => {
        const agora = performance.now();
        if (agora - ultima >= a.ms || agora - inicio >= a.max) {
          obs.disconnect();
          resolve({ quieto: agora - ultima >= a.ms, doc: meuDoc });
        } else {
          setTimeout(checar, 50);
        }
      };
      setTimeout(checar, 50);
    });
  }

  return {
    v: __VERSAO__,
    el: (a) => exigir(a.ref),
    achar: (a) => !!achar(a.ref),
    doc: () => meuDoc,
    mapa, localizar, ponto, caixaQuadro, tipo, preparar, definir, estado, escolher, opcoes,
    clicarJs, focar, temTexto, api, temSel, porTexto, quieto, formLogin, etapaUsuario, liberarSenhaSalva, rolagem,
  };
})()
