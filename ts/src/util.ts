import { Decorator, FormApi } from 'final-form';
import Cookie from 'universal-cookie';


// localDefault returns def locally otherwise undefined.
export const ifLocal = <T>(def: T): T | undefined => (
  process.env.NODE_ENV === 'development' ? def : undefined
);

// localDefault returns def locally otherwise undefined.
export const localOrDefault = <T>(local: T, not: T): T => (
  process.env.NODE_ENV === 'development' ? local : not
);

export const withCookiePersistence = <FormValues>(persistedValues: (keyof FormValues)[]): Decorator<FormValues> => (form: FormApi<FormValues>) => form.subscribe(
  ({ values }: { values?: FormValues }) => {
    if (values === undefined) {
      return;
    }

    new Cookie('persist').set('persist', JSON.stringify(
      persistedValues.reduce((acc, k) => ({ ...acc, [k]: values[k] }), {})
    ));
  },
  { values: true },
);

