const rubFormatter = new Intl.NumberFormat('ru-RU', { style: 'currency', currency: 'RUB' });
const dateFormatter = new Intl.DateTimeFormat('ru-RU', { dateStyle: 'medium', timeStyle: 'short' });

export function formatTipAmount(amountMinor: number, currency: string): string {
  if (currency !== 'RUB' || !Number.isSafeInteger(amountMinor) || amountMinor <= 0 || amountMinor > 1_000_000) return 'Сумма недоступна';
  return rubFormatter.format(amountMinor / 100);
}

export function formatTipDate(value: string): string {
  const date = new Date(value);
  return Number.isNaN(date.getTime()) ? 'Дата недоступна' : dateFormatter.format(date);
}
