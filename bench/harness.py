#!/usr/bin/env python3
"""Bancada de comparação: quem decide cada passo do `nd` (Sonnet via `claude -p`,
pplx-decider via OpenRouter, ou os dois em híbrido) em 3 sites locais com telas escondidas.

Uso:
  python3 harness.py --arms sonnet,decider --reps 3 [--tasks vagas-1,banco-1] [--out results.jsonl]
  python3 harness.py --report results.jsonl
Variáveis: OPENROUTER_API_KEY, DECIDER_URL (padrão https://openrouter.ai/api/alpha/decisions),
DECIDER_MODEL (padrão perplexity/pplx-decider-v1.2), SONNET_MODEL (padrão claude-sonnet-5-5),
ND_BIN, ND_DIR, BENCH_PORT.
"""
import argparse, json, os, re, statistics, subprocess, sys, tempfile, time, urllib.request, urllib.error
from pathlib import Path

HERE = Path(__file__).resolve().parent
_env = Path(__file__).resolve().parent / ".env"
if _env.exists():
    for _l in _env.read_text().splitlines():
        if "=" in _l and not _l.startswith("#"):
            _k, _v = _l.split("=", 1)
            if _v.strip():
                os.environ.setdefault(_k.strip(), _v.strip())

ND_BIN = os.environ.get("ND_BIN", str(HERE.parent / "target/release/nd"))
PORT = int(os.environ.get("BENCH_PORT", "8765"))
BASE = f"http://127.0.0.1:{PORT}"
MAX_PASSOS = int(os.environ.get("BENCH_MAX_PASSOS", "40"))
SONNET_MODEL = os.environ.get("SONNET_MODEL", "claude-sonnet-5-5")
DECIDER_URL = os.environ.get("DECIDER_URL", "https://openrouter.ai/api/alpha/decisions")
DECIDER_MODEL = os.environ.get("DECIDER_MODEL", "perplexity/pplx-decider-v1.2")
LIMIAR_HIBRIDO = float(os.environ.get("BENCH_LIMIAR", "0.6"))

ACOES = {
    "clicar": "clicar no elemento alvo",
    "preencher": "digitar o valor no campo alvo",
    "escolher": "escolher a opção (valor) num select ou lista do elemento alvo",
    "marcar": "marcar o checkbox, radio ou switch alvo",
    "desmarcar": "desmarcar o checkbox ou switch alvo",
    "rolar": "rolar a página para baixo",
    "concluir": "a meta já foi cumprida; não há mais nada a fazer",
}


# ---------------------------------------------------------------- tarefas

def _n(s):
    return re.sub(r"\D", "", str(s))


def chk_vagas1(e):
    c = e["candidaturas"]
    return (len(c) == 1 and c[0]["vaga"] == 31 and _n(c[0]["telefone"]) == "81988887777"
            and c[0]["email"] == "trabalho@exemplo.com" and c[0]["cv"] == "cv-2026.pdf"
            and str(c[0]["sql"]) == "5" and _n(c[0]["pretensao"]) == "8000"
            and c[0]["imediato"] == "sim" and c[0]["seguir"] is False and e["seguindo"] == [])


def chk_vagas2(e):
    a = e["alertas"]
    return e["salvas"] == [15] and a["semelhantes"] is True and a["frequencia"] == "semanal" and a["email"] is False


def chk_banco1(e):
    t = e["transferencias"]
    return (e["logado"] and len(t) == 1 and t[0]["destId"] == 2 and abs(t[0]["valor"] - 250) < 0.001
            and t[0]["data"] == "2026-11-15" and t[0]["descricao"].strip().lower() == "aluguel")


def chk_banco2(e):
    c = {x["final"]: x for x in e["cartoes"]}
    return (e["logado"] and c["4417"]["bloqueado"] is True and c["4417"]["limite"] == 3000
            and not c["8820"]["bloqueado"] and not c["1093"]["bloqueado"]
            and c["8820"]["limite"] == 0 and c["1093"]["limite"] == 500)


def chk_sup1(e):
    t = {x["id"]: x for x in e["tickets"]}
    a = t[4821]
    return (a["resp"] == "Paula Reis" and a["prio"] == "Alta" and a["status"] == "Fechado"
            and a.get("motivo") == "Resolvido"
            and [r.strip() for r in a.get("respostas", [])] == ["Estorno aprovado, prazo de 5 dias úteis."]
            and t[4803]["status"] == "Aberto" and t[4827]["status"] == "Aberto"
            and t[4803]["resp"] == "" and t[4827]["resp"] == "")


def chk_sup2(e):
    return e["notificacoes"] == {"resumoDiario": False, "alertaSla": True, "slaHoras": 4}


TASKS = {
    "vagas-1": dict(site="vagas", check=chk_vagas1,
        objetivo="Candidate-se, pela candidatura simples, à vaga Analista de Dados da empresa Nuvem Azul em Recife, modalidade Remoto, nível Pleno. Use os dados fornecidos. Não siga a empresa.",
        dados={"telefone": "81 98888-7777", "e-mail": "trabalho@exemplo.com", "currículo": "cv-2026.pdf",
               "anos de SQL": "5", "pretensão salarial": "8000", "início imediato": "Sim"}),
    "vagas-2": dict(site="vagas", check=chk_vagas2,
        objetivo="Salve a vaga Engenheiro de Dados da empresa Rio Tech em São Paulo (Remoto). Depois, em Minhas vagas > Alertas, ative o alerta de vagas semelhantes com frequência semanal, sem e-mail.",
        dados={"frequência": "Semanal"}),
    "banco-1": dict(site="banco", check=chk_banco1,
        objetivo="Entre na conta e faça um Pix agendado de R$ 250,00 para Marina Costa (não a Marina Costa Lima) para o dia 15 de novembro, com a descrição Aluguel. Autorize com o token que o próprio banco mostra.",
        dados={"agência": "0412", "conta": "77120-5", "senha": "135790", "valor": "250", "descrição": "Aluguel", "destinatário": "Marina Costa"}),
    "banco-2": dict(site="banco", check=chk_banco2,
        objetivo="Entre na conta, bloqueie temporariamente o cartão de crédito Gold final 4417 e defina o limite dele para R$ 3000. Não altere os outros cartões.",
        dados={"agência": "0412", "conta": "77120-5", "senha": "135790", "limite": "3000"}),
    "suporte-1": dict(site="suporte", check=chk_sup1,
        objetivo="Abra o ticket #4821 (Cobrança duplicada), atribua a Paula Reis, mude a prioridade para Alta, responda ao cliente com o texto dado e feche o ticket com o motivo Resolvido.",
        dados={"texto da resposta": "Estorno aprovado, prazo de 5 dias úteis.", "motivo": "Resolvido"}),
    "suporte-2": dict(site="suporte", check=chk_sup2,
        objetivo="Em Configurações > Notificações > E-mail, desligue o Resumo diário e ligue o Alerta de SLA com antecedência de 4 horas.",
        dados={"antecedência": "4 horas"}),
}


# ---------------------------------------------------------------- sites reais (só leitura / sem conta pessoal)
TXT = "JSON.stringify({url: location.href, texto: document.body.innerText})"


def chk_sauce(e):
    t = e["texto"]
    return ("checkout-step-two" in e["url"] and "Sauce Labs Backpack" in t and "Sauce Labs Bike Light" in t
            and not any(x in t for x in ("Bolt T-Shirt", "Fleece Jacket", "Onesie", "Test.allTheThings")))


def chk_demoqa(e):
    t = e["texto"]
    return all(x in t for x in ("Thanks for submitting the form", "Arthur Teste", "arthur@teste.com", "Male", "9999999999",
                                "15 March,1990", "Maths", "Sports", "Rua A, 10", "NCR Delhi"))


def chk_books(e):
    return "sapiens-a-brief-history-of-humankind" in e["url"]


def chk_internet(e):
    return e["sem_checkbox"] and e["habilitado"] and e["valor"] == "ola mundo"


TASKS.update({
    "real-sauce": dict(url="https://www.saucedemo.com", estado_js=TXT, check=chk_sauce,
        objetivo="Entre na loja, adicione ao carrinho SOMENTE os produtos Sauce Labs Backpack e Sauce Labs Bike Light, vá ao checkout, preencha os dados e chegue na tela de resumo do pedido (Checkout: Overview). Não finalize o pedido.",
        dados={"usuário": "standard_user", "senha": "secret_sauce", "primeiro nome": "Arthur", "sobrenome": "Teste", "cep": "50000"}),
    "real-demoqa": dict(url="https://demoqa.com/automation-practice-form", estado_js=TXT, check=chk_demoqa,
        objetivo="Preencha o formulário com os dados fornecidos (gênero Male, data de nascimento 15 de março de 1990, matéria Maths, hobby Sports, estado NCR e cidade Delhi) e envie. O envio mostra um diálogo de confirmação.",
        dados={"first name": "Arthur", "last name": "Teste", "email": "arthur@teste.com", "celular": "9999999999",
               "mês de nascimento": "March", "ano de nascimento": "1990", "matéria": "Maths", "endereço": "Rua A, 10",
               "estado": "NCR", "cidade": "Delhi"}),
    "real-books": dict(url="https://books.toscrape.com", estado_js=TXT, check=chk_books,
        objetivo="Abra a página de detalhe do livro 'Sapiens: A Brief History of Humankind'.",
        dados={"título": "Sapiens: A Brief History of Humankind"}),
    "real-internet": dict(url="https://the-internet.herokuapp.com/dynamic_controls",
        estado_js="JSON.stringify({sem_checkbox: !document.querySelector('#checkbox'), habilitado: !document.querySelector('#input-example input').disabled, valor: document.querySelector('#input-example input').value})",
        check=chk_internet,
        objetivo="Remova o checkbox, depois habilite o campo de texto e digite o texto dado nele.",
        dados={"texto": "ola mundo"}),
})


# ---------------------------------------------------------------- nd

def nd(*args, timeout=60):
    t0 = time.perf_counter()
    p = subprocess.run([ND_BIN, *args], capture_output=True, text=True, timeout=timeout)
    return p.returncode, (p.stdout + p.stderr).strip(), (time.perf_counter() - t0) * 1000


def estado_site(tarefa=None):
    js = (tarefa or {}).get("estado_js", "JSON.stringify(window.__estado)")
    rc, out, _ = nd("eval", js)
    try:
        return json.loads(out)
    except Exception:
        return None


def executar(dec):
    a, alvo, valor = dec.get("acao"), dec.get("alvo"), dec.get("valor")
    if a in ("preencher", "escolher") and not valor:
        return 1, "erro: ação sem valor", 0.0
    if a == "clicar": return nd("click", alvo)
    if a == "preencher": return nd("fill", alvo, str(valor))
    if a == "escolher": return nd("pick", alvo, str(valor))
    if a == "marcar": return nd("check", alvo)
    if a == "desmarcar": return nd("check", alvo, "--nao")
    if a == "rolar": return nd("scroll")
    return 1, f"ação desconhecida {a!r}", 0.0


# ---------------------------------------------------------------- braços

SYS = ("Você controla um navegador por comandos. A cada turno recebe o objetivo, os dados permitidos, o histórico "
       "e o mapa atual da página (linhas 'eN tipo \"nome\"'; só eN pode ser alvo). Responda SOMENTE com um objeto JSON: "
       '{"acao": clicar|preencher|escolher|marcar|desmarcar|rolar|concluir, "alvo": "eN", "valor": "texto"}. '
       "'valor' só em preencher e escolher. Um passo por resposta. Use 'concluir' apenas quando o objetivo estiver cumprido.")


def prompt_usuario(tarefa, hist, mapa, ultima=""):
    h = "\n".join(f"{i+1}. {x}" for i, x in enumerate(hist[-12:])) or "(nenhum)"
    return (f"OBJETIVO: {tarefa['objetivo']}\nDADOS: {json.dumps(tarefa['dados'], ensure_ascii=False)}\n"
            f"HISTÓRICO:\n{h}\n\nSAÍDA COMPLETA DA ÚLTIMA AÇÃO:\n{ultima or '(nenhuma)'}\n\nMAPA ATUAL:\n{mapa}\n")


def decide_sonnet(tarefa, hist, mapa, ultima=""):
    t0 = time.perf_counter()
    cwd = Path(tempfile.gettempdir()) / "bench-cwd"
    cwd.mkdir(exist_ok=True)
    p = subprocess.run(
        ["claude", "-p", "--model", SONNET_MODEL, "--output-format", "json", "--tools", "",
         "--no-session-persistence", "--disable-slash-commands", "--strict-mcp-config",
         "--mcp-config", '{"mcpServers":{}}', "--setting-sources", "project", "--system-prompt", SYS],
        input=prompt_usuario(tarefa, hist, mapa, ultima), capture_output=True, text=True, cwd=cwd, timeout=300)
    ms = (time.perf_counter() - t0) * 1000
    m = dict(chamadas=1, ms=ms, tok_in=0, tok_out=0, tok_cache=0, custo=0.0, erro=None)
    try:
        j = json.loads(p.stdout)
        u = j.get("usage", {})
        m["tok_cache"] = u.get("cache_creation_input_tokens", 0) + u.get("cache_read_input_tokens", 0)
        m["tok_in"] = u.get("input_tokens", 0) + m["tok_cache"]   # entrada total, com cache
        m["tok_out"] = u.get("output_tokens", 0)
        m["custo"] = j.get("total_cost_usd") or 0.0
        m["api_ms"] = j.get("duration_api_ms")
        txt = j.get("result", "")
        mm = re.search(r"\{.*\}", txt, re.S)
        return json.loads(mm.group(0)), m
    except Exception as e:
        m["erro"] = f"{e}: {(p.stdout or p.stderr)[:200]}"
        return {"acao": "concluir"}, m


def candidatos_valor(tarefa, mapa, ultima=""):
    c = {f"{k}": f"{k} = {v}" for k, v in tarefa["dados"].items()}
    for n in sorted(set(re.findall(r"\b\d{4,8}\b", mapa + " " + ultima))):
        c[f"num:{n}"] = f"número visível na página: {n}"
    c["nenhum"] = "nenhum texto (ação sem valor)"
    return c


CONFERE = ("Antes de escolher a próxima ação, confira a saida_da_ultima_acao contra o objetivo: os nomes, números e "
           "identificadores que o objetivo cita (por exemplo o número do ticket aberto, o nome completo do destinatário) "
           "coincidem com o que a saída mostra? Se algo contradiz o objetivo, a próxima ação deve corrigir isso (voltar, "
           "escolher outro item) em vez de seguir em frente.")


def _decidir_uma(tarefa, hist, mapa, ultima, verificar, alerta=None):
    """Uma chamada ao Decider. Com `verificar`, junta a pergunta 'confere' (sim/não) à mesma requisição."""
    key = os.environ.get("OPENROUTER_API_KEY")
    if not key:
        raise SystemExit("defina OPENROUTER_API_KEY")
    refs = {}
    for ln in mapa.splitlines():
        mm = re.match(r"\s*(e\d+)\s+(.*)", ln)
        if mm:
            refs[mm.group(1)] = mm.group(2)[:140]
    cand = candidatos_valor(tarefa, mapa, ultima)
    state = {"objetivo": tarefa["objetivo"], "dados": tarefa["dados"], "historico": hist[-12:], "saida_da_ultima_acao": ultima, "mapa_da_pagina": mapa}
    if alerta:
        state["alerta"] = alerta
    acao_instr = "Qual é a próxima ação única para avançar o objetivo, dado o histórico e o mapa? Não repita uma ação que já deu certo."
    if verificar and ultima:
        state["conferencia"] = CONFERE
        acao_instr += " Siga a 'conferencia' do state."
    questions = {
        "acao": {"type": "choice", "instructions": acao_instr, "criteria": ACOES},
        "alvo": {"type": "choice", "instructions": "Qual elemento do mapa recebe a próxima ação? (Ignore se a ação for rolar ou concluir.)", "criteria": refs or {"e1": "nenhum"}},
        "valor": {"type": "choice", "instructions": "Qual dado deve ser digitado ou escolhido na próxima ação? Use 'nenhum' se a ação não leva texto.", "criteria": cand},
    }
    body = {"model": DECIDER_MODEL, "state": state, "questions": questions}
    req = urllib.request.Request(DECIDER_URL, json.dumps(body).encode(), {"Authorization": f"Bearer {key}", "Content-Type": "application/json"})
    t0 = time.perf_counter()
    m = dict(chamadas=1, ms=0, tok_in=0, tok_out=0, tok_cache=0, custo=0.0, erro=None)
    try:
        with urllib.request.urlopen(req, timeout=120) as r:
            j = json.loads(r.read())
    except urllib.error.HTTPError as e:
        m["ms"] = (time.perf_counter() - t0) * 1000
        m["erro"] = f"HTTP {e.code}: {e.read()[:300]!r}"
        return {"acao": "concluir"}, m
    m["ms"] = (time.perf_counter() - t0) * 1000
    u = j.get("usage", {})
    m["tok_in"], m["tok_out"] = u.get("input_tokens", 0), u.get("output_tokens", 0)
    m["custo"] = u.get("cost") or 0.0
    a = j["answers"]
    dec = {"acao": a["acao"]["choice"], "alvo": a["alvo"]["choice"], "conf": min(a["acao"].get("confidence", 1), a["alvo"].get("confidence", 1))}
    v = a["valor"]["choice"]
    if v != "nenhum":
        dec["valor"] = v.split(":", 1)[1] if v.startswith("num:") else tarefa["dados"].get(v, "")
    return dec, m


def decide_decider(tarefa, hist, mapa, ultima=""):
    return _decidir_uma(tarefa, hist, mapa, ultima, verificar=False)


def decide_decider_v(tarefa, hist, mapa, ultima=""):
    """Decider com a conferência no contexto: mesma requisição, sem pergunta nem chamada extra."""
    return _decidir_uma(tarefa, hist, mapa, ultima, verificar=True)


def decide_hibrido(tarefa, hist, mapa, ultima=""):
    dec, m = decide_decider(tarefa, hist, mapa, ultima)
    if m["erro"] or dec.get("conf", 0) < LIMIAR_HIBRIDO:
        d2, m2 = decide_sonnet(tarefa, hist, mapa, ultima)
        for k in ("chamadas", "ms", "tok_in", "tok_out", "tok_cache", "custo"):
            m[k] += m2[k]
        m["escalou"] = 1
        return d2, m
    return dec, m


ARMS = {"sonnet": decide_sonnet, "decider": decide_decider, "decider_v": decide_decider_v, "hibrido": decide_hibrido}


# ---------------------------------------------------------------- execução

def rodar(tid, arm, rep):
    tarefa = TASKS[tid]
    decide = ARMS[arm]
    t_ini = time.perf_counter()
    nd_ms = 0.0
    url = tarefa.get("url") or f"{BASE}/{tarefa['site']}/index.html?t={time.time_ns()}"
    rc, mapa, ms = nd("open", url, "--sem-map")
    nd_ms += ms
    hist, passos, ultima, tot = [], [], "", dict(chamadas=0, ms=0.0, tok_in=0, tok_out=0, tok_cache=0, escaladas=0, reprovacoes=0, custo=0.0)
    fim = "limite"
    for i in range(MAX_PASSOS):
        rc, mapa, ms = nd("map")
        nd_ms += ms
        dec, m = decide(tarefa, hist, mapa, ultima)
        for k in ("chamadas", "ms", "tok_in", "tok_out", "tok_cache", "custo"):
            tot[k] += m[k]
        tot["escaladas"] += m.get("escalou", 0)
        tot["reprovacoes"] += m.get("reprovou", 0)
        if m["erro"]:
            fim = "erro_api"; passos.append(dict(dec=dec, erro=m["erro"])); break
        if dec.get("acao") == "concluir":
            fim = "concluiu"; passos.append(dict(dec=dec)); break
        rc, out, ms = executar(dec)
        nd_ms += ms
        ultima = out[:800]
        resumo = f"{dec.get('acao')} {dec.get('alvo','')} {dec.get('valor','')}".strip()
        hist.append(f"{resumo} -> {'ok' if rc == 0 else 'ERRO'}: {out.splitlines()[0] if out else ''}"[:200])
        passos.append(dict(dec=dec, rc=rc, ms=round(m['ms']), saida=out[:160]))
        # repetição idêntica três vezes: o agente está preso
        if len(hist) >= 4 and len({h.split(' ->')[0] for h in hist[-3:]}) == 1:
            fim = "preso"; break
    total_ms = (time.perf_counter() - t_ini) * 1000
    e = estado_site(tarefa)
    ok = bool(e and tarefa["check"](e))
    return dict(tarefa=tid, braco=arm, rep=rep, sucesso=ok, fim=fim, passos=len(passos), total_ms=round(total_ms),
                modelo_ms=round(tot["ms"]), nd_ms=round(nd_ms), chamadas_api=tot["chamadas"], tok_in=tot["tok_in"],
                tok_out=tot["tok_out"], tok_cache=tot["tok_cache"], escaladas=tot["escaladas"], reprovacoes=tot["reprovacoes"], custo_usd=round(tot["custo"], 6), trilha=passos)


def relatorio(arq):
    linhas = [json.loads(x) for x in Path(arq).read_text().splitlines() if x.strip()]
    med = lambda xs: round(statistics.median(xs)) if xs else 0
    print(f"{'tarefa':<10} {'braço':<8} {'n':>2} {'ok':>5} {'tempo s':>8} {'modelo s':>9} {'nd s':>6} {'passos':>6} {'chamadas':>8} {'tok_in':>8} {'tok_out':>7} {'tok_cache':>9} {'US$':>8}")
    chaves = sorted({(l["tarefa"], l["braco"]) for l in linhas})
    for t, b in chaves:
        g = [l for l in linhas if l["tarefa"] == t and l["braco"] == b]
        ok = sum(l["sucesso"] for l in g)
        print(f"{t:<10} {b:<8} {len(g):>2} {ok:>2}/{len(g):<2} {med([l['total_ms'] for l in g])/1000:>8.1f} {med([l['modelo_ms'] for l in g])/1000:>9.1f} "
              f"{med([l['nd_ms'] for l in g])/1000:>6.1f} {med([l['passos'] for l in g]):>6} {med([l['chamadas_api'] for l in g]):>8} "
              f"{med([l['tok_in'] for l in g]):>8} {med([l['tok_out'] for l in g]):>7} {med([l['tok_cache'] for l in g]):>9} {statistics.median([l.get('custo_usd',0) for l in g]):>8.4f}")
    print("\n(medianas por tarefa; tokens e chamadas são por execução completa)")
    for b in sorted({l["braco"] for l in linhas}):
        g = [l for l in linhas if l["braco"] == b]
        print(f"TOTAL {b:<8} execuções {len(g)}  sucesso {sum(l['sucesso'] for l in g)}  tempo {sum(l['total_ms'] for l in g)/1000:.0f}s  "
              f"chamadas {sum(l['chamadas_api'] for l in g)}  tok_in {sum(l['tok_in'] for l in g)}  tok_out {sum(l['tok_out'] for l in g)}  tok_cache {sum(l['tok_cache'] for l in g)}  custo US$ {sum(l.get('custo_usd',0) for l in g):.4f}")


if __name__ == "__main__":
    ap = argparse.ArgumentParser()
    ap.add_argument("--arms", default="sonnet,decider")
    ap.add_argument("--tasks", default=",".join(TASKS))
    ap.add_argument("--reps", type=int, default=3)
    ap.add_argument("--out", default=str(HERE / "results.jsonl"))
    ap.add_argument("--report")
    ap.add_argument("--braco-refazer", help="com --refazer: roda as falhas neste braço em vez do original")
    ap.add_argument("--refazer", help="results.jsonl: refaz só as execuções que falharam nele")
    a = ap.parse_args()
    if a.report:
        relatorio(a.report); sys.exit()
    if a.refazer:
        falhas = [json.loads(x) for x in Path(a.refazer).read_text().splitlines() if x.strip()]
        falhas = [(l["tarefa"], l["braco"], l["rep"]) for l in falhas if not l["sucesso"]]
        print(f"refazendo {len(falhas)} execuções que falharam", flush=True)
        for t, arm, rep in falhas:
            r = rodar(t, a.braco_refazer or arm, rep)
            with open(a.out, "a") as f:
                f.write(json.dumps(r, ensure_ascii=False) + "\n")
            print(f"{t} {arm} rep{rep}: {'OK' if r['sucesso'] else 'FALHOU'} ({r['fim']}) {r['total_ms']/1000:.1f}s passos={r['passos']} custo=US${r.get('custo_usd',0):.4f} chamadas={r['chamadas_api']} reprov={r.get('reprovacoes',0)} tok_in={r['tok_in']} tok_out={r['tok_out']}", flush=True)
        relatorio(a.out); sys.exit()
    arms, tasks = a.arms.split(","), a.tasks.split(",")
    for rep in range(a.reps):          # braços intercalados dentro de cada repetição
        for t in tasks:
            for arm in arms:
                r = rodar(t, arm, rep)
                with open(a.out, "a") as f:
                    f.write(json.dumps(r, ensure_ascii=False) + "\n")
                print(f"{t} {arm} rep{rep}: {'OK' if r['sucesso'] else 'FALHOU'} ({r['fim']}) {r['total_ms']/1000:.1f}s passos={r['passos']} custo=US${r.get('custo_usd',0):.4f} chamadas={r['chamadas_api']} reprov={r.get('reprovacoes',0)} tok_in={r['tok_in']} tok_out={r['tok_out']}", flush=True)
    relatorio(a.out)
