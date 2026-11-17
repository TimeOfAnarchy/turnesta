# Сквозные браузерные тесты

Тесты создают организацию и владельца через admin CLI, затем проходят путь сотрудника и двух посетителей через интерфейс. Для каждого браузера создаётся отдельная организация. Локальный HTTP-сервер отдаёт страницу клиники с кодом виджета, скопированным из кабинета.

Запускайте из корня отдельной рабочей копии. Нужны Docker Compose и Node.js 22.12 или новее. `COMPOSE_PROJECT_NAME` должен указывать на тестовый стенд, а `E2E_BASE_URL` совпадать с его `TURNESTA_PUBLIC_ORIGIN`. Пример для Linux или Git Bash с отдельными портами:

```sh
export COMPOSE_PROJECT_NAME=turnesta-e2e
export TURNESTA_API_PORT=18080 TURNESTA_POSTGRES_PORT=55432
export TURNESTA_PUBLIC_ORIGIN=http://127.0.0.1:18080
export E2E_BASE_URL="$TURNESTA_PUBLIC_ORIGIN"
./scripts/init-env.sh
docker compose up --build --wait api
cd e2e
npm ci
npx playwright install --with-deps chromium firefox webkit
npm test
```

Один браузер можно запустить командой `npx playwright test --project=firefox`. При повторном прогоне создаются новые тестовые данные. Чтобы завершить работу и удалить данные этого тестового проекта, вернитесь в корень и выполните `docker compose down --volumes` с тем же `COMPOSE_PROJECT_NAME`.
