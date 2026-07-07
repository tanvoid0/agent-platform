import logging
import os
from contextlib import asynccontextmanager
from pathlib import Path

from fastapi import Depends, FastAPI, Request
from fastapi.middleware.cors import CORSMiddleware
from fastapi.responses import HTMLResponse, RedirectResponse
from fastapi.templating import Jinja2Templates

from action_orchestrator import router as action_orchestrator_router
from api_tokens import AVAILABLE_SCOPES
from api_tokens.auth import require_valid_token
from api_tokens.legacy_routes import router as api_tokens_legacy_router
from api_tokens.routes import router as api_tokens_router
from chat_routes import router as chat_router
from database import create_db_and_tables
from llm_proxy.admin_routes import router as llm_proxy_admin_router
from llm_proxy.core.errors import register_exception_handlers
from llm_proxy.core.middleware import RequestIdMiddleware
from llm_proxy.routes.llm import router as llm_proxy_router
from llm_proxy.services.model_catalog_cache import get_catalog_cache
from llm_proxy_env import llm_proxy_master_key
from process_routes import router as process_router
from projects_routes import router as projects_router
from workspaces_routes import router as workspaces_router, me_router as me_workspace_router
from teams_routes import router as teams_router
from todos.routes import router as todos_router
from assistant.routes import router as assistant_router
from playground.routes import router as playground_router
from coder.routes import router as coder_router
from workspace_routes import files_router as workspace_files_router, router as workspace_router

logger = logging.getLogger(__name__)


@asynccontextmanager
async def lifespan(app: FastAPI):
    create_db_and_tables()
    if not llm_proxy_master_key():
        logger.warning(
            "AGENT_PLATFORM_MASTER_KEY is not set; LLM proxy /v1 calls and planner chat will fail until it is set."
        )
    cache = get_catalog_cache()
    await cache.start_background_refresh()
    # Resume processes stranded mid-plan/mid-run by the previous shutdown; executors
    # are in-process asyncio tasks and do not survive a restart.
    from services.startup_recovery import schedule_startup_recovery

    schedule_startup_recovery()
    try:
        yield
    finally:
        await cache.stop_background_refresh()


_env = (os.getenv("AGENT_PLATFORM_ENV") or "development").strip().lower()
app = FastAPI(
    title="Agent Platform",
    version="0.1.0",
    lifespan=lifespan,
    docs_url=None if _env == "production" else "/docs",
    redoc_url=None if _env == "production" else "/redoc",
    openapi_url="/openapi.json",  # stays on in all envs; the frontend's own docs UI depends on it
)
register_exception_handlers(app)
app.add_middleware(RequestIdMiddleware)
app.include_router(llm_proxy_router)

_api_deps = [Depends(require_valid_token)]
# Routers at root paths for backward compatibility
app.include_router(process_router, dependencies=_api_deps)
app.include_router(teams_router, dependencies=_api_deps)
app.include_router(projects_router, dependencies=_api_deps)
app.include_router(workspaces_router, dependencies=_api_deps)
app.include_router(me_workspace_router, dependencies=_api_deps)
app.include_router(workspace_router, dependencies=_api_deps)
app.include_router(workspace_files_router, dependencies=_api_deps)
app.include_router(action_orchestrator_router, dependencies=_api_deps)
app.include_router(api_tokens_router, dependencies=_api_deps)
app.include_router(api_tokens_legacy_router, dependencies=_api_deps)
# Same routers mirrored under /api/v1 (versioned REST surface)
app.include_router(process_router, prefix="/api/v1", dependencies=_api_deps)
app.include_router(teams_router, prefix="/api/v1", dependencies=_api_deps)
app.include_router(projects_router, prefix="/api/v1", dependencies=_api_deps)
app.include_router(workspaces_router, prefix="/api/v1", dependencies=_api_deps)
app.include_router(me_workspace_router, prefix="/api/v1", dependencies=_api_deps)
app.include_router(workspace_router, prefix="/api/v1", dependencies=_api_deps)
app.include_router(workspace_files_router, prefix="/api/v1", dependencies=_api_deps)
app.include_router(action_orchestrator_router, prefix="/api/v1", dependencies=_api_deps)
app.include_router(api_tokens_router, prefix="/api/v1", dependencies=_api_deps)
app.include_router(api_tokens_legacy_router, prefix="/api/v1", dependencies=_api_deps)
# Additional routers at /api/v1 prefix
app.include_router(todos_router, prefix="/api/v1", dependencies=_api_deps)
app.include_router(assistant_router, prefix="/api/v1", dependencies=_api_deps)
app.include_router(playground_router, prefix="/api/v1", dependencies=_api_deps)
app.include_router(coder_router, prefix="/api/v1", dependencies=_api_deps)
app.include_router(chat_router, prefix="/api/v1", dependencies=_api_deps)
app.include_router(llm_proxy_admin_router, prefix="/api/v1/llm-proxy", dependencies=_api_deps)

_cors_origins = [
    o.strip() for o in os.getenv("CORS_ALLOW_ORIGINS", "*").split(",") if o.strip()
] or ["*"]
# Browsers reject Access-Control-Allow-Origin: * together with Allow-Credentials: true.
_cors_allow_credentials = "*" not in _cors_origins
app.add_middleware(
    CORSMiddleware,
    allow_origins=_cors_origins,
    allow_credentials=_cors_allow_credentials,
    allow_methods=["*"],
    allow_headers=["*"],
)

BASE_DIR = Path(__file__).resolve().parent
templates = Jinja2Templates(directory=str(BASE_DIR / "templates"))


@app.get("/", include_in_schema=False)
def root():
    return RedirectResponse(url="/config")


@app.get("/health")
def health():
    return {"status": "ok", "service": "agent-platform"}


@app.get("/config", response_class=HTMLResponse, include_in_schema=False)
def config_page(request: Request):
    return templates.TemplateResponse("config.html", {"request": request})


@app.get("/ui", response_class=HTMLResponse, include_in_schema=False)
def ui_page(request: Request):
    return templates.TemplateResponse(
        "index.html",
        {
            "request": request,
            "api_base": os.getenv("AGENT_PLATFORM_PUBLIC_API", "").strip() or "",
        },
    )


@app.get("/tokens", response_class=HTMLResponse, include_in_schema=False)
def tokens_page(request: Request):
    return templates.TemplateResponse("tokens.html", {"request": request})


@app.get("/api/v1/api-tokens/scopes", tags=["api-tokens"], dependencies=_api_deps)
def list_available_scopes():
    """Catalog of scopes a token can be granted (for dashboard autocomplete)."""
    return {"scopes": AVAILABLE_SCOPES}


@app.get("/api-guide", response_class=HTMLResponse, include_in_schema=False)
def api_guide_page(request: Request):
    return templates.TemplateResponse("api_guide.html", {"request": request})
