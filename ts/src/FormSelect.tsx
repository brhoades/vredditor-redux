import React from 'react';
import Form from 'react-bootstrap/Form';
import { FieldRenderProps } from 'react-final-form';

export default ({
  input,
  meta,
  ...extraProps
}: FieldRenderProps<string>) => {
  const error = meta.error || meta.submitError;
  const showError = meta.invalid && !meta.dirtySinceLastSubmit && meta.touched && !meta.active;
  const { type, ...remInput } = input;

  return (
    <>
      <Form.Control
        isInvalid={showError}
        as="select"
        {...remInput}
        {...extraProps}
      />
      {
        showError && (
          <Form.Control.Feedback type="invalid">
            { error }
          </Form.Control.Feedback>
        )
      }
    </>
  );
};
