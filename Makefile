# ═══════════════════════════════════════════════════════════════
# BASTION v3 — Makefile
# ═══════════════════════════════════════════════════════════════

.PHONY: help up down restart logs pull update test test-skills clean

help: ## Mostra este menu
	@grep -E '^[a-zA-Z_-]+:.*?## .*$$' $(MAKEFILE_LIST) | \
		awk 'BEGIN {FS = ":.*?## "}; {printf "\033[36m%-15s\033[0m %s\n", $$1, $$2}'

# ── Operação ──────────────────────────────────────────────────

up: ## Sobe o Bastion (core daemon + skills)
	docker compose up -d

down: ## Para o Bastion
	docker compose down

restart: ## Reinicia sem recriar containers
	docker compose restart

logs: ## Acompanha os logs em tempo real
	docker compose logs -f core

# ── Atualização ───────────────────────────────────────────────

pull: ## Baixa as imagens mais recentes
	docker compose pull

update: pull ## Atualiza e reinicia com as novas imagens
	docker compose up -d

# ── Testes ────────────────────────────────────────────────────

test: ## Roda todos os property tests dos skills
	python3 -m pytest skills/ -q

test-skills: ## Roda testes de um skill específico (make test-skills SKILL=weight-system)
	python3 -m pytest skills/$(SKILL)/tests/ -v --rootdir=.

# ── Limpeza ───────────────────────────────────────────────────

clean: ## Remove containers e volumes (⚠️ apaga dados locais)
	docker compose down -v --remove-orphans
