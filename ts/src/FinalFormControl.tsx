import React from 'react';
import Form from 'react-bootstrap/Form';
import { FieldRenderProps } from 'react-final-form';

export default ({
  input,
  meta,
  children,
  ...extraProps
}: FieldRenderProps<string>) => {
  const error = meta.error || meta.submitError;
  const showError = meta.invalid && !meta.dirtySinceLastSubmit && meta.touched && !meta.active;
  return (
    <>
      <Form.Control
        isInvalid={showError}
        {...input}
        {...extraProps}
      >
        { children }
      </Form.Control>
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
