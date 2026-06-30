import logging
import os
from contextlib import asynccontextmanager
from pathlib import Path

from fastapi import Depends, FastAPI, Request
from fastapi.middleware.cors import CORSMiddleware
from fastapi.responses import HTMLResponse, RedirectResponse
from fastapi.templating import Jinja2Templates

from action_orchestrator import router as action_orchestrator_router
from api_auth import verify_agent_platform_api_key
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
from teams_routes import router as teams_router
from todos.routes import router as todos_router
from assistant.routes import router as assistant_router
from workspace_routes import router as workspace_router

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
    try:
        yield
    finally:
        await cache.stop_background_refresh()


app = FastAPI(title="Agent Platform", version="0.1.0", lifespan=lifespan)
register_exception_handlers(app)
app.add_middleware(RequestIdMiddleware)
app.include_router(llm_proxy_router)

_api_deps = [Depends(verify_agent_platform_api_key)]
# Routers at root paths for backward compatibility
app.include_router(process_router, dependencies=_api_deps)
app.include_router(teams_router, dependencies=_api_deps)
app.include_router(projects_router, dependencies=_api_deps)
app.include_router(workspace_router, dependencies=_api_deps)
app.include_router(action_orchestrator_router, dependencies=_api_deps)
# Additional routers at /api/v1 prefix
app.include_router(todos_router, prefix="/api/v1", dependencies=_api_deps)
app.include_router(assistant_router, prefix="/api/v1", dependencies=_api_deps)
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


@app.get("/api-guide", response_class=HTMLResponse, include_in_schema=False)
def api_guide_page(request: Request):
    return templates.TemplateResponse("api_guide.html", {"request": request})
