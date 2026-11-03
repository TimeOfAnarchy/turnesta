// Native time inputs follow the browser locale (12-hour on en-US); the cabinet always uses HH:MM.
export const time24Pattern = '([01][0-9]|2[0-3]):[0-5][0-9]';
const complete = new RegExp(`^${time24Pattern}$`);

// Formats typed or pasted text as HH:MM; value is null until the time is complete and valid.
export function typedTime(raw: string): { text: string; value: string | null } {
  let digits = raw.replace(/\D/g, '').slice(0, 4);
  if (/^\d\D/.test(raw)) digits = `0${digits}`.slice(0, 4);
  const text = digits.length > 2 ? `${digits.slice(0, 2)}:${digits.slice(2)}` : digits;
  return { text, value: complete.test(text) ? text : null };
}
