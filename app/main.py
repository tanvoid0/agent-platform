import logging
import os
from contextlib import asynccontextmanager
from pathlib import Path

from fastapi import Depends, FastAPI, Request
from fastapi.responses import HTMLResponse, JSONResponse
from fastapi.templating import Jinja2Templates

from action_orchestrator import router as action_orchestrator_router
from api_tokens import AVAILABLE_SCOPES
from api_tokens.auth import require_valid_token
from api_tokens.routes import router as api_tokens_router
from chat_routes import router as chat_router
from database import create_db_and_tables
from llm_proxy.admin_routes import router as llm_proxy_admin_router
from llm_proxy.core.errors import register_exception_handlers
from llm_proxy.core.middleware import RequestIdMiddleware
from llm_proxy.routes.llm import router as llm_proxy_router
from llm_proxy.services.model_catalog_cache import get_catalog_cache
from health_checks import app_readiness_payload
from llm_proxy_env import llm_proxy_master_key
from observability import RequestLoggingMiddleware, setup_logging
from process_routes import router as process_router
from projects_routes import router as projects_router
from startup_validation import assert_startup_config
from system_routes import router as system_router
from workspaces_routes import router as workspaces_router, me_router as me_workspace_router
from teams_routes import router as teams_router
from todos.routes import router as todos_router
from assistant.routes import router as assistant_router
from playground.routes import router as playground_router
from coder.routes import router as coder_router
from model_ops.routes import router as model_ops_router
from workspace_routes import files_router as workspace_files_router, router as workspace_router

setup_logging()
logger = logging.getLogger(__name__)


@asynccontextmanager
async def lifespan(app: FastAPI):
    assert_startup_config()
    create_db_and_tables()
    if not llm_proxy_master_key():
        logger.info(
            "AGENT_PLATFORM_MASTER_KEY is not set; protected /v1 and planner chat endpoints will stay unavailable."
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
app.add_middleware(RequestLoggingMiddleware)
app.include_router(llm_proxy_router)

_api_deps = [Depends(require_valid_token)]
# The versioned REST surface is the only one; the bare-root mirror was removed
# with the browser UI (the native desktop client targets /api/v1 exclusively).
app.include_router(process_router, prefix="/api/v1", dependencies=_api_deps)
app.include_router(teams_router, prefix="/api/v1", dependencies=_api_deps)
app.include_router(projects_router, prefix="/api/v1", dependencies=_api_deps)
app.include_router(workspaces_router, prefix="/api/v1", dependencies=_api_deps)
app.include_router(me_workspace_router, prefix="/api/v1", dependencies=_api_deps)
app.include_router(workspace_router, prefix="/api/v1", dependencies=_api_deps)
app.include_router(workspace_files_router, prefix="/api/v1", dependencies=_api_deps)
app.include_router(action_orchestrator_router, prefix="/api/v1", dependencies=_api_deps)
app.include_router(api_tokens_router, prefix="/api/v1", dependencies=_api_deps)
# Additional routers at /api/v1 prefix
app.include_router(todos_router, prefix="/api/v1", dependencies=_api_deps)
app.include_router(assistant_router, prefix="/api/v1", dependencies=_api_deps)
app.include_router(playground_router, prefix="/api/v1", dependencies=_api_deps)
app.include_router(coder_router, prefix="/api/v1", dependencies=_api_deps)
app.include_router(model_ops_router, prefix="/api/v1", dependencies=_api_deps)
app.include_router(chat_router, prefix="/api/v1", dependencies=_api_deps)
app.include_router(llm_proxy_admin_router, prefix="/api/v1/llm-proxy", dependencies=_api_deps)
app.include_router(system_router, prefix="/api/v1", dependencies=_api_deps)

BASE_DIR = Path(__file__).resolve().parent
templates = Jinja2Templates(directory=str(BASE_DIR / "templates"))


@app.get("/", include_in_schema=False)
def root():
    return {"service": "agent-platform", "api": "/api/v1", "docs": app.docs_url}


@app.get("/health")
def health():
    return {"status": "ok", "service": "agent-platform"}


@app.get("/ready")
def ready():
    status_code, payload = app_readiness_payload()
    return JSONResponse(status_code=status_code, content=payload)


@app.get("/tokens", response_class=HTMLResponse, include_in_schema=False)
def tokens_page(request: Request):
    return templates.TemplateResponse("tokens.html", {"request": request})


@app.get("/api/v1/api-tokens/scopes", tags=["api-tokens"], dependencies=_api_deps)
def list_available_scopes():
    """Catalog of scopes a token can be granted (for dashboard autocomplete)."""
    return {"scopes": AVAILABLE_SCOPES}
