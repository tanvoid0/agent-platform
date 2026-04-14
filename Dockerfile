FROM python:3.11-slim

WORKDIR /app

COPY app/requirements.txt .
RUN pip install --no-cache-dir -r requirements.txt

COPY app/ .

RUN mkdir -p /app/data/llm

EXPOSE 18410

CMD ["uvicorn", "main:app", "--host", "0.0.0.0", "--port", "18410"]
