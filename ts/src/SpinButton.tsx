import React from 'react';

import Button from 'react-bootstrap/Button';
import Spinner from 'react-bootstrap/Spinner';

type Props = {
  loading: boolean;
  loadingMessage: string;
  message: string;
  spinnerProps?: Partial<React.ComponentProps<typeof Spinner>>;
} & React.ComponentProps<typeof Button>;

export default ({
  loading,
  loadingMessage,
  message,
  spinnerProps = {},
  ...buttonProps
}: Props) => (
  <Button
    {...buttonProps}
  >
    {
      loading && (
        <Spinner
          size="sm"
          animation="border"
          role="status"
          aria-hidden="true"
          {...spinnerProps}
        />
      )
    }
    { loading ? loadingMessage : message }
  </Button>
);
